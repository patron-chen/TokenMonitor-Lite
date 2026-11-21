mod index;
#[cfg(test)]
mod tests;

use std::{collections::{HashMap, HashSet}, sync::atomic::AtomicBool};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::{
    pricing::Pricing, quota_history::{self, same_reset, QuotaHistory, QuotaKey, Reading, RETENTION_MS},
    settings::ToolSelection, sources::Sources,
};
use super::{QuotaWindow, ToolQuota};

const MIN_POINTS: f64 = 5.0;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Forecast
{
    basis: &'static str,
    observed_at: i64,
    used_tokens: Option<u64>,
    used_usd: Option<f64>,
    missing_price: bool,
    partial: bool,
    total_tokens: Option<u64>,
    total_usd: Option<f64>,
}

#[derive(Clone, Copy, Default)]
struct Amount
{
    tokens: u64,
    usd: Option<f64>,
    missing: bool,
}

impl Amount
{
    fn add(&mut self, other: Self)
    {
        self.tokens += other.tokens;
        if let Some(cost) = other.usd { *self.usd.get_or_insert(0.0) += cost; }
        self.missing |= other.missing;
    }
}

#[derive(Default)]
struct Calibration
{
    amount: Amount,
    points: f64,
}

#[derive(Default)]
struct Ledger
{
    groups: HashMap<String, Vec<(i64, Amount)>>,
    gaps: Vec<(i64, i64)>,
    since: i64,
    through: i64,
}

fn model_group<'a>(tool: &str, model: &str, groups: &'a [String]) -> Option<&'a str>
{
    if tool == "codex" { return Some(""); }
    let model = model.to_ascii_lowercase();
    let labels: &[&str] = if model.contains("argon") { &["Gemini 4 Argon", "Gemini"] }
        else if model.contains("gemini") && model.contains("flash") { &["Gemini Flash", "Gemini"] }
        else if model.contains("gemini") { &["Gemini Pro", "Gemini"] }
        else if model.contains("claude") || model.contains("gpt") { &["Claude/GPT"] }
        else { &[] };
    labels.iter().find_map(|label| groups.iter().find(|group| group.eq_ignore_ascii_case(label)))
        .or_else(|| groups.iter().find(|group| !model.is_empty() && group.eq_ignore_ascii_case(&model)))
        .map(String::as_str)
        .or_else(|| labels.last().copied())
}

// Use the first confirmed observation of the new identity after a known account/plan change.
fn scope_start(db: &Connection, tool: &str, cutoff: i64) -> rusqlite::Result<i64>
{
    let mut since = cutoff;
    for field in ["plan", "account"]
    {
        let sql = format!("WITH identified AS (SELECT at,{field} AS value FROM quota_profiles
            WHERE tool=?1 AND {field} IS NOT NULL), latest AS (SELECT value FROM identified ORDER BY at DESC LIMIT 1)
            SELECT MIN(at) FROM identified WHERE value=(SELECT value FROM latest)
            AND at>(SELECT MAX(at) FROM identified WHERE value<>(SELECT value FROM latest))");
        let boundary: Option<i64> = db.query_row(&sql, [tool], |row| row.get(0))?;
        since = since.max(boundary.unwrap_or(cutoff));
    }
    Ok(since)
}

impl Ledger
{
    fn load(db: &Connection, provider: &ToolQuota, now: i64) -> rusqlite::Result<Option<Self>>
    {
        let tool = provider.tool;
        let Some(through) = db.query_row("SELECT at FROM forecast_scans WHERE tool=?1", [tool], |r| r.get(0)).optional()?
            else { return Ok(None) };
        let since = scope_start(db, tool, now - RETENTION_MS)?;
        let mut ledger = Self { since, through, ..Self::default() };
        let groups: Vec<_> = provider.windows.iter().map(|window| window.group.clone()).collect();
        let mut seen = HashSet::new();
        let mut query = db.prepare("SELECT event,at,model,tokens,usd,missing FROM forecast_requests
            WHERE tool=?1 AND at BETWEEN ?2 AND ?3 ORDER BY at,path")?;
        let mut rows = query.query(params![tool, since, through])?;
        while let Some(row) = rows.next()?
        {
            if !seen.insert(row.get::<_, String>(0)?) { continue; }
            let at = row.get(1)?;
            if let Some(group) = model_group(tool, &row.get::<_, String>(2)?, &groups)
            {
                ledger.groups.entry(group.to_owned()).or_default().push((at, Amount {
                    tokens: row.get(3)?, usd: row.get(4)?, missing: row.get(5)?,
                }));
            }
            else { ledger.gaps.push((at, at)); }
        }
        let mut gaps = db.prepare("SELECT gap_start,gap_end FROM forecast_files WHERE tool=?1 AND gap_end>=?2")?;
        for gap in gaps.query_map(params![tool, since], |row| Ok((row.get(0)?, row.get(1)?)))?
        {
            ledger.gaps.push(gap?);
        }
        ledger.gaps.sort_unstable();
        // Coalesce overlapping gaps so range lookups stay cheap on partially readable sources.
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (start, end) in ledger.gaps
        {
            if let Some(last) = merged.last_mut().filter(|last| last.1 >= start) { last.1 = last.1.max(end); }
            else { merged.push((start, end)); }
        }
        ledger.gaps = merged;
        Ok(Some(ledger))
    }

    fn amount(&self, group: &str, start: i64, end: i64) -> (Amount, bool)
    {
        let mut amount = Amount::default();
        if let Some(events) = self.groups.get(group)
        {
            let first = events.partition_point(|(at, _)| *at < start);
            for (_, item) in events[first..].iter().take_while(|(at, _)| *at <= end) { amount.add(*item); }
        }
        let first_gap = self.gaps.partition_point(|(_, right)| *right < start);
        let partial = self.gaps.get(first_gap).is_some_and(|(left, _)| *left <= end);
        (amount, partial)
    }
}

fn estimate(window: &QuotaWindow, observed: i64, readings: &[Reading], ledger: &Ledger, now: i64) -> Option<Forecast>
{
    let minutes = window.minutes.filter(|n| matches!(n, 300 | 10_080))?;
    let duration = minutes as i64 * 60_000;
    let end = window.resets_at?;
    let mut start = end.checked_sub(duration)?;
    if now >= end || observed < start || observed >= end || observed < ledger.since { return None; }
    // A full refill can reuse the same reset timestamp. Its usage starts with the refill.
    for pair in readings.windows(2)
    {
        if pair[1].at <= observed && pair[1].at >= start && pair[1].value == 100.0 && pair[0].value < 100.0 &&
            [pair[0], pair[1]].iter().all(|p| p.reset_at.is_some_and(|reset| same_reset(reset, end)))
        {
            start = pair[1].at;
        }
    }
    let mut current = Calibration::default();
    let mut historical = Calibration::default();
    let mut anchor: Option<Reading> = None;
    // A manual reset starts a new calibration at the next observed reading, even across cycles.
    // Cycle usage still includes all locally recorded requests since the actual quota reset.
    let calibration_start = window.forecast_reset_at.unwrap_or(ledger.since).max(ledger.since);
    for &point in readings.iter().filter(|p| p.at >= calibration_start && p.at <= ledger.through && p.at <= observed)
    {
        let Some(reset) = point.reset_at.filter(|reset| point.at < *reset && point.at >= reset - duration) else
        {
            anchor = None;
            continue;
        };
        let prior = anchor.replace(point);
        let Some(prior) = prior else { continue };
        let prior_reset = prior.reset_at?;
        let first = if same_reset(prior_reset, reset)
        {
            if point.value == prior.value
            {
                // Keep usage accumulated while a rounded percentage is on a plateau.
                anchor = Some(prior);
                continue;
            }
            prior.at + 1
        }
        else if prior.value == 100.0 && point.value < 100.0 &&
            same_reset(prior_reset - duration, prior.at) && reset - duration <= prior_reset
        {
            // At full quota Antigravity advances resetAt on each poll until the first spend.
            (prior.at + 1).max(reset - duration)
        }
        else { continue };
        let drop = prior.value - point.value;
        if drop <= 0.0 || first > point.at { continue; }
        let target = if same_reset(reset, end) && first >= start { &mut current }
            else if point.at < start { &mut historical } else { continue };
        let (amount, gap) = ledger.amount(&window.group, first, point.at);
        if !gap && amount.tokens > 0
        {
            target.points += drop;
            target.amount.add(amount);
        }
    }
    let through = ledger.through.min(now);
    let (used, gap) = ledger.amount(&window.group, start.max(ledger.since), through);
    let calibration = if current.points + 1e-9 >= MIN_POINTS { Some(("current", current)) }
        else if historical.points + 1e-9 >= MIN_POINTS { Some(("history", historical)) } else { None };
    let mut result = Forecast {
        basis: "insufficient", observed_at: through, used_tokens: (through >= start).then_some(used.tokens),
        used_usd: used.usd.or_else(|| (!used.missing && through >= start).then_some(0.0)), missing_price: used.missing,
        partial: gap || ledger.since > start, total_tokens: None, total_usd: None,
    };
    if let Some((basis, calibration)) = calibration
    {
        let factor = 100.0 / calibration.points;
        result.basis = basis;
        result.total_tokens = Some((calibration.amount.tokens as f64 * factor).round() as u64);
        let missing_price = if used.missing && calibration_start > start.max(ledger.since)
        {
            ledger.amount(&window.group, calibration_start, through).0.missing
        }
        else { used.missing };
        if !missing_price && !calibration.amount.missing { result.total_usd = calibration.amount.usd.map(|usd| usd * factor); }
    }
    Some(result)
}

pub(super) fn attach(history: &QuotaHistory, providers: &mut [ToolQuota], tools: ToolSelection, now: i64)
{
    let mut load = || -> rusqlite::Result<()> {
        let mut db = history.connection()?;
        db.execute_batch(index::SCHEMA)?;
        let tx = db.transaction()?;
        let mut resets = tx.prepare("SELECT at FROM forecast_resets WHERE series=?1 AND at>=?2")?;
        for provider in providers.iter_mut().filter(|p| tools.contains(p.tool) && !p.windows.is_empty())
        {
            let ledger = Ledger::load(&tx, provider, now)?;
            for window in &mut provider.windows
            {
                window.forecast = None;
                if !matches!(window.minutes, Some(300 | 10_080)) { continue; }
                let key = QuotaKey::Window { tool: provider.tool.into(), group: window.group.clone(), minutes: window.minutes };
                window.forecast_reset_at = resets.query_row(params![key.identity(), now - RETENTION_MS], |row| row.get(0)).optional()?;
                let Some(ledger) = &ledger else { continue };
                window.forecast = estimate(window, provider.observed_at, &quota_history::read(&tx, &key, now)?, ledger, now);
            }
        }
        Ok(())
    };
    if let Err(error) = load() { eprintln!("Could not read quota forecasts: {error}"); }
}

pub(super) fn reset(history: &QuotaHistory, key: &QuotaKey, window: &mut QuotaWindow, now: i64) -> rusqlite::Result<()>
{
    let db = history.connection()?;
    db.execute_batch(index::SCHEMA)?;
    db.execute("INSERT INTO forecast_resets (series,at) VALUES (?1,?2)
        ON CONFLICT(series) DO UPDATE SET at=excluded.at", params![key.identity(), now])?;
    window.forecast_reset_at = Some(now);
    if let Some(forecast) = &mut window.forecast
    {
        forecast.basis = "insufficient";
        forecast.total_tokens = None;
        forecast.total_usd = None;
    }
    Ok(())
}

pub(super) fn observe_account(history: &QuotaHistory, sources: &Sources, now: i64)
{
    if let Some(account) = index::current_codex_account(sources)
    {
        let _ = history.connection().and_then(|db| quota_history::store_profile(&db, "codex", now, None, Some(&account)));
    }
}

pub(super) fn sync(
    history: &QuotaHistory, sources: &Sources, tool: &str, now: i64, stopped: &AtomicBool,
) -> Result<Option<ToolQuota>, Box<dyn std::error::Error>>
{
    use std::sync::OnceLock;
    static PRICES: OnceLock<Pricing> = OnceLock::new();
    let mut db = history.connection()?;
    db.execute_batch(index::SCHEMA)?;
    index::sync(&mut db, sources, tool, PRICES.get_or_init(Pricing::load), now, stopped)?;
    if tool != "codex" { return Ok(None); }
    // An archived or already-indexed rollout may still be the newest quota after restart.
    let mut latest = Vec::new();
    for minutes in [300, 10_080]
    {
        let key = QuotaKey::Window { tool: tool.into(), group: String::new(), minutes: Some(minutes) };
        if let Some(reading) = quota_history::read(&db, &key, now)?.last().copied() { latest.push((minutes, reading)); }
    }
    let Some(at) = latest.iter().map(|(_, r)| r.at).max() else { return Ok(None) };
    let plan = db.query_row("SELECT plan FROM quota_profiles WHERE tool=?1 AND at<=?2 AND plan IS NOT NULL ORDER BY at DESC LIMIT 1",
        params![tool, at], |row| row.get(0)).optional()?;
    let windows = latest.into_iter().filter(|(_, r)| r.at == at).map(|(minutes, r)| QuotaWindow {
        group: String::new(), minutes: Some(minutes), remaining: r.value, resets_at: r.reset_at, forecast_reset_at: None, forecast: None,
    }).collect();
    Ok(Some(ToolQuota { tool: "codex", plan, account: None, source: None, observed_at: at, windows, balances: vec![], banked_reset: None }))
}
