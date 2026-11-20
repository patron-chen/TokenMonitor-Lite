use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    quota_history::{HistoryPreview, QuotaHistory, QuotaKey, Reading, Sample, RETENTION_MS},
    settings::{QuotaSettings, ToolSelection},
    sources::Sources,
};

#[cfg(windows)]
mod antigravity;
mod banked_log;
use banked_log::BankedResetLog;

#[cfg(all(windows, test))]
mod proxy_tests;

#[cfg(windows)]
const QUOTA_SCRIPT: &str = concat!(include_str!("../../scripts/windows-http.ps1"), "\n", include_str!("quota_windows.ps1"));
// One 20s HTTP request plus time for PowerShell startup and teardown.
#[cfg(windows)]
const REMOTE_HELPER_TIMEOUT: Duration = Duration::from_secs(30);

const CODEX_TAIL_BYTES: u64 = 2 * 1024 * 1024;
const CODEX_HISTORY_LIMIT: usize = 1200;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quotas
{
    checked_at: Option<i64>,
    observed_at: i64,
    tools: Vec<ToolQuota>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolQuota
{
    tool: &'static str,
    plan: Option<String>,
    observed_at: i64,
    windows: Vec<QuotaWindow>,
    balances: Vec<Balance>,
    banked_reset: Option<BankedReset>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuotaWindow
{
    group: String,
    minutes: Option<u64>,
    remaining: f64,
    resets_at: Option<i64>,
}

#[derive(Clone, Serialize)]
struct Balance
{
    currency: &'static str,
    amount: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BankedReset
{
    available_count: u64,
    expirations: Vec<i64>,
}

#[derive(Default)]
pub struct QuotaCache
{
    snapshot: Mutex<Quotas>,
    refreshed: Mutex<Option<(Instant, ToolSelection)>>,
    history: Option<QuotaHistory>,
    banked_log: BankedResetLog,
}

impl QuotaCache
{
    pub fn new(root: &Path) -> Self
    {
        Self { history: Some(QuotaHistory::new(root)), banked_log: BankedResetLog::new(root), ..Self::default() }
    }

    pub fn cached(&self, tools: ToolSelection) -> Option<Quotas>
    {
        let snapshot = self.snapshot.lock().expect("quota snapshot");
        snapshot.checked_at.map(|_| snapshot.for_tools(tools))
    }

    pub fn read(&self, sources: &Sources, settings: QuotaSettings, force: bool) -> Quotas
    {
        let tools = settings.tools;
        if !tools.any() { return Quotas::default(); }
        self.refresh(settings, force, || std::thread::scope(|scope| {
            let codex = tools.codex.then(|| scope.spawn(|| {
                let cutoff = self.history.as_ref().map(|_| {
                    self.snapshot.lock().expect("quota snapshot").tools.iter()
                        .find(|quota| quota.tool == "codex" && !quota.windows.is_empty())
                        .map_or_else(|| Utc::now().timestamp_millis() - RETENTION_MS, |quota| quota.observed_at)
                });
                codex_quota(&sources.codex_sessions(), cutoff)
            }));
            let providers = if sources.home_override.is_none()
            {
                windows_quota(&sources.codex_auth(), tools, &self.banked_log)
            }
            else
            {
                if tools.codex { self.banked_log.write(format_args!("request skipped=source_home_override")); }
                Providers::default()
            };
            let (codex, history) = codex.map(|reader| reader.join().expect("Codex quota reader")).unwrap_or_default();
            (codex, providers, history)
        }))
    }

    pub fn history(&self, key: &QuotaKey) -> Result<HistoryPreview, String>
    {
        self.history.as_ref().ok_or("Quota history is unavailable.")?.preview(key)
            .map_err(|_| "Could not read the local quota history.".into())
    }

    pub fn record_banked_reset_panel(&self, limits_view: bool, codex_enabled: bool, read_failed: bool, count: Option<u64>, rendered: bool)
    {
        let view = if limits_view { "limits" } else { "home" };
        let count = count.map_or_else(|| "unavailable".into(), |value| value.to_string());
        self.banked_log.write(format_args!("panel view={view} codex_enabled={codex_enabled} read_failed={read_failed} count={count} rendered={rendered}"));
    }

    fn refresh(
        &self, settings: QuotaSettings, force: bool,
        collect: impl FnOnce() -> (Option<ToolQuota>, Providers, Vec<Sample>),
    ) -> Quotas
    {
        // Opening a WebView can read the last snapshot while I/O is in flight.
        let mut refreshed = self.refreshed.lock().expect("quota refresh");
        let ttl = Duration::from_secs(u64::from(settings.refresh_minutes) * 60);
        self.banked_log.begin(format_args!("refresh force={force} codex_enabled={}", settings.tools.codex));
        if !force && refreshed.is_some_and(|(at, tools)| tools == settings.tools && at.elapsed() < ttl)
        {
            self.banked_log.write(format_args!("refresh result=cache_hit"));
            return self.cached(settings.tools).unwrap_or_default();
        }
        let start = Instant::now();
        let (codex, providers, mut samples) = collect();
        let observed_at = Utc::now().timestamp_millis();
        let mut snapshot = self.snapshot.lock().expect("quota snapshot").clone();
        let bank_source = snapshot.merge(codex, providers, observed_at, &mut samples);
        if let Some(history) = &self.history
        {
            if history.record(&samples, observed_at).is_err() { eprintln!("Could not save the local quota history."); }
        }
        *refreshed = Some((Instant::now(), settings.tools));
        let result = snapshot.for_tools(settings.tools);
        *self.snapshot.lock().expect("quota snapshot") = snapshot;
        if settings.tools.codex
        {
            let bank = result.tools.iter().find(|row| row.tool == "codex").and_then(|row| row.banked_reset.as_ref());
            let count = bank.map_or_else(|| "unavailable".into(), |bank| bank.available_count.to_string());
            let expirations = bank.map_or(0, |bank| bank.expirations.len());
            self.banked_log.write(format_args!("quota source={bank_source} count={count} future_expirations={expirations} elapsed_ms={}", start.elapsed().as_millis()));
        }
        result
    }
}

impl Quotas
{
    fn for_tools(&self, tools: ToolSelection) -> Self
    {
        let mut result = self.clone();
        result.tools.retain(|quota| tools.contains(quota.tool));
        result
    }

    fn merge(&mut self, codex: Option<ToolQuota>, providers: Providers, now: i64, samples: &mut Vec<Sample>) -> &'static str
    {
        self.checked_at = Some(now);
        let previous = |tool| self.tools.iter().find(|quota| quota.tool == tool).cloned();
        let previous_codex = previous("codex");
        let antigravity = providers.antigravity;
        let deepseek = deepseek_quota(&providers.deepseek, now);
        let bank = parse_banked_reset(&providers.codex_reset, now);
        // Only successful numeric observations enter the history, before fallback to cached rows.
        for quota in [&codex, &antigravity, &deepseek].into_iter().flatten() { quota.samples(samples); }
        if antigravity.is_some() || deepseek.is_some() || codex.is_some() || bank.is_some()
        {
            self.observed_at = now;
        }
        let previous_bank = previous_codex.as_ref().and_then(|quota| quota.banked_reset.as_ref());
        let bank_source = if bank.is_some() { "response" } else if previous_bank.is_some() { "previous" } else { "none" };
        let bank = bank.or_else(|| previous_bank.cloned());
        let codex = add_banked_reset(codex.or(previous_codex), bank, now);
        self.tools = [antigravity.or_else(|| previous("antigravity")), codex,
            deepseek.or_else(|| previous("dsh"))].into_iter().flatten().collect();
        bank_source
    }
}

impl ToolQuota
{
    fn samples(&self, output: &mut Vec<Sample>)
    {
        output.extend(self.windows.iter().map(|window| Sample {
            key: QuotaKey::Window { tool: self.tool.into(), group: window.group.clone(), minutes: window.minutes },
            reading: Reading { at: self.observed_at, value: window.remaining, reset_at: window.resets_at },
        }));
        output.extend(self.balances.iter().map(|balance| Sample {
            key: QuotaKey::Balance { tool: self.tool.into(), currency: balance.currency.into() },
            reading: Reading { at: self.observed_at, value: balance.amount, reset_at: None },
        }));
    }
}

fn rollout_candidates(path: &Path, depth: u8, files: &mut Vec<(SystemTime, PathBuf)>)
{
    for entry in fs::read_dir(path).into_iter().flatten().filter_map(Result::ok)
    {
        let file = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() && depth > 0
        {
            rollout_candidates(&file, depth - 1, files);
        }
        else if kind.is_file() && file.extension().is_some_and(|ext| ext == "jsonl")
            && entry.file_name().to_string_lossy().starts_with("rollout-")
        {
            if let Ok(updated) = entry.metadata().and_then(|meta| meta.modified()) { files.push((updated, file)); }
        }
    }
}

fn codex_quota(root: &Path, history_after: Option<i64>) -> (Option<ToolQuota>, Vec<Sample>)
{
    let mut files = Vec::new();
    rollout_candidates(root, 3, &mut files);
    files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut history = Vec::new();
    let latest = files.iter().take(8).filter_map(|(_, path)| {
        let mut minute = None;
        read_codex_tail(path, |quota| {
            let bucket = quota.observed_at / 60_000;
            // Recover samples written between refreshes from the existing bounded log reads.
            if history_after.is_some_and(|after| quota.observed_at > after)
                && history.len() < CODEX_HISTORY_LIMIT && minute != Some(bucket)
            {
                quota.samples(&mut history);
                minute = Some(bucket);
            }
        })
    }).max_by_key(|row| row.observed_at);
    (latest, history)
}

fn read_codex_tail(path: &Path, mut observe: impl FnMut(&ToolQuota)) -> Option<ToolQuota>
{
    let file = File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(CODEX_TAIL_BYTES);
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    if start > 0
    {
        reader.seek(SeekFrom::Start(start - 1)).ok()?;
        reader.read_until(b'\n', &mut line).ok()?;
    }
    let mut latest = None;
    loop
    {
        line.clear();
        if reader.read_until(b'\n', &mut line).ok()? == 0 { break; }
        if !line.windows(b"\"rate_limits\"".len()).any(|part| part == b"\"rate_limits\"") { continue; }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else { continue };
        if let Some(quota) = parse_codex(&value)
        {
            observe(&quota);
            latest = Some(quota);
        }
    }
    latest
}

fn parse_codex(value: &Value) -> Option<ToolQuota>
{
    let payload = &value["payload"];
    if value["type"] != "event_msg" || payload["type"] != "token_count" { return None; }
    let limits = &payload["rate_limits"];
    if limits["limit_id"].as_str().is_some_and(|id| id != "codex") { return None; }
    let observed_at = reset_time(&value["timestamp"])?;
    let plan = limits["plan_type"].as_str().filter(|name| !name.is_empty()).map(|name| match name
    {
        "prolite" => "Pro 100",
        "pro" => "Pro 200",
        "promax" => "Pro 500",
        "plus" => "Plus",
        "free" => "Free",
        other => other,
    }.to_owned());
    let mut windows: Vec<_> = ["primary", "secondary"].into_iter().filter_map(|key| {
        let window = &limits[key];
        Some(QuotaWindow {
            group: String::new(), minutes: window["window_minutes"].as_u64(),
            remaining: (100.0 - window["used_percent"].as_f64()?).clamp(0.0, 100.0),
            resets_at: reset_time(&window["resets_at"]),
        })
    }).collect();
    windows.sort_by_key(|window| window.minutes);
    (!windows.is_empty()).then_some(ToolQuota {
        tool: "codex", plan, observed_at, windows, balances: vec![], banked_reset: None,
    })
}

fn add_banked_reset(codex: Option<ToolQuota>, banked_reset: Option<BankedReset>, now: i64) -> Option<ToolQuota>
{
    let Some(banked_reset) = banked_reset else { return codex };
    let mut quota = codex.unwrap_or_else(|| ToolQuota {
        tool: "codex", plan: None, observed_at: now, windows: vec![], balances: vec![], banked_reset: None,
    });
    quota.banked_reset = Some(banked_reset);
    Some(quota)
}

fn parse_banked_reset(value: &Value, now: i64) -> Option<BankedReset>
{
    let count = &value["available_count"];
    let available_count = count.as_u64().or_else(|| count.as_str()?.parse().ok())?;
    let mut expirations: Vec<_> = array(&value["credits"]).iter().filter_map(|credit| {
        credit["status"].as_str()?.eq_ignore_ascii_case("available")
            .then(|| reset_time(&credit["expires_at"])).flatten().filter(|expiry| *expiry > now)
    }).collect();
    expirations.sort_unstable();
    Some(BankedReset { available_count, expirations })
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Providers
{
    #[serde(skip)]
    antigravity: Option<ToolQuota>,
    codex_reset: Value,
    deepseek: Value,
}

fn array(value: &Value) -> &[Value]
{
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn reset_time(value: &Value) -> Option<i64>
{
    if let Some(value) = value.as_i64()
    {
        return (value > 0).then(|| if value >= 1_000_000_000_000 { value } else { value * 1000 });
    }
    let value = value.as_str()?;
    DateTime::parse_from_rfc3339(value).ok().map(|time| time.timestamp_millis())
}

fn window_minutes(bucket: &Value) -> Option<u64>
{
    for name in ["window", "bucketId", "displayName"].into_iter().filter_map(|key| bucket[key].as_str())
    {
        let name = name.trim().to_ascii_lowercase().replace('_', "-");
        let name = name.strip_suffix(" limit").unwrap_or(&name);
        if name == "weekly" || name.ends_with("-weekly") { return Some(10_080); }
        if ["session", "5h", "5-hour", "five hour", "five-hour"].iter()
            .any(|alias| name == *alias || name.strip_suffix(alias).is_some_and(|prefix| prefix.ends_with('-')))
        {
            return Some(300);
        }
    }
    None
}

fn percent(fraction: f64) -> f64 { (fraction * 100.0).clamp(0.0, 100.0) }

fn antigravity_quota(summary: &Value, configs: &Value, observed_at: i64) -> Option<ToolQuota>
{
    let summary = [&summary["response"], &summary["summary"], summary].into_iter()
        .find(|value| value.is_object()).unwrap_or(summary);
    let mut windows = Vec::new();
    for group in array(&summary["groups"])
    {
        let label = group["displayName"].as_str().unwrap_or("Quota").trim();
        let lower = label.to_ascii_lowercase();
        let label = match lower.as_str()
        {
            "gemini models" => "Gemini",
            _ if lower.contains("claude") || lower.contains("gpt") => "Claude/GPT",
            _ => label,
        };
        for bucket in array(&group["buckets"])
        {
            if bucket["disabled"] == true { continue; }
            let remaining = &bucket["remaining"];
            let fraction = bucket["remainingFraction"].as_f64()
                .or_else(|| remaining["remainingFraction"].as_f64())
                .or_else(|| (remaining["case"] == "remainingFraction").then(|| remaining["value"].as_f64()).flatten());
            if let (Some(fraction), Some(minutes)) = (fraction, window_minutes(bucket))
            {
                windows.push(QuotaWindow { group: label.to_owned(), minutes: Some(minutes),
                    remaining: percent(fraction), resets_at: reset_time(&bucket["resetTime"]) });
            }
        }
    }
    if windows.is_empty()
    {
        for cfg in array(configs)
        {
            let Some(id) = cfg["modelOrAlias"]["model"].as_str() else { continue };
            // Legacy model lists also advertise retired placeholders outside the active quota pools.
            if matches!(id, "" | "MODEL_CHAT_20706" | "MODEL_CHAT_23310" | "MODEL_GOOGLE_GEMINI_2_5_FLASH"
                | "MODEL_GOOGLE_GEMINI_2_5_FLASH_THINKING" | "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE"
                | "MODEL_GOOGLE_GEMINI_2_5_PRO" | "MODEL_PLACEHOLDER_M19" | "MODEL_PLACEHOLDER_M9" | "MODEL_PLACEHOLDER_M12")
            {
                continue;
            }
            let label = cfg["label"].as_str().unwrap_or(id);
            let family = format!("{label} {id}").to_ascii_lowercase();
            let group = if family.contains("argon") { "Gemini 4 Argon" }
                else if family.contains("gemini") && family.contains("flash") { "Gemini Flash" }
                else if family.contains("gemini") { "Gemini Pro" }
                else { "Claude/GPT" };
            let quota = &cfg["quotaInfo"];
            let Some(fraction) = quota["remainingFraction"].as_f64() else { continue };
            let value = QuotaWindow { group: group.to_owned(), minutes: None, remaining: percent(fraction),
                resets_at: reset_time(&quota["resetTime"]) };
            if let Some(existing) = windows.iter_mut().find(|window| window.group == group)
            {
                if value.remaining < existing.remaining { *existing = value; }
            }
            else { windows.push(value); }
        }
    }
    windows.sort_by_key(|window| {
        let rank = if window.group.contains("Argon") { 1 } else if window.group == "Claude/GPT" { 2 } else { 0 };
        (rank, window.minutes)
    });
    (!windows.is_empty()).then_some(ToolQuota {
        tool: "antigravity", plan: None, observed_at, windows, balances: vec![], banked_reset: None,
    })
}

fn deepseek_quota(data: &Value, observed_at: i64) -> Option<ToolQuota>
{
    let balances: Vec<_> = array(&data["balance_infos"]).iter().filter_map(|record| {
        let currency = match record["currency"].as_str()? { "USD" => "USD", "CNY" => "CNY", _ => return None };
        let amount: f64 = record["total_balance"].as_str()?.parse().ok()?;
        amount.is_finite().then_some(Balance { currency, amount })
    }).collect();
    (!balances.is_empty()).then_some(ToolQuota {
        tool: "dsh", plan: None, observed_at, windows: vec![], balances, banked_reset: None,
    })
}

#[cfg(windows)]
fn windows_quota(codex_auth: &Path, tools: ToolSelection, log: &BankedResetLog) -> Providers
{
    std::thread::scope(|scope| {
        let remote = |tool: &str| -> Providers {
            let log_path = log.path.as_deref().filter(|_| tool == "codex").map(Path::as_os_str).unwrap_or_default();
            powershell_json(QUOTA_SCRIPT, &[
                ("TOKEN_MONITOR_LITE_CODEX_AUTH", codex_auth.as_os_str()),
                ("TOKEN_MONITOR_LITE_QUOTA_TOOLS", tool.as_ref()),
                ("TOKEN_MONITOR_LITE_BANKED_LOG", log_path),
            ], REMOTE_HELPER_TIMEOUT).unwrap_or_else(|reason| {
                if tool == "codex" { log.write(format_args!("helper failure={reason}")); }
                eprintln!("Windows {tool} quota helper failed: {reason}");
                Providers::default()
            })
        };
        let local = tools.antigravity.then(|| scope.spawn(antigravity::read));
        let read_balance = tools.dsh && ["DEEPSEEK_API_KEY", "DEEPSEEK_KEY"].iter()
            .any(|name| std::env::var_os(name).is_some_and(|key| !key.is_empty()));
        // A slow balance request must not consume the reset request's deadline or discard its result.
        let balance = read_balance.then(|| scope.spawn(move || remote("dsh").deepseek));
        let codex_reset = if tools.codex { remote("codex").codex_reset } else { Value::Null };
        Providers {
            antigravity: local.and_then(|worker| worker.join().expect("Antigravity quota reader")),
            codex_reset,
            deepseek: balance.map(|worker| worker.join().expect("DeepSeek quota reader")).unwrap_or_default(),
        }
    })
}

#[cfg(windows)]
fn powershell_json<T: serde::de::DeserializeOwned + Send>(
    script: &str, vars: &[(&str, &std::ffi::OsStr)], timeout: Duration,
) -> Result<T, String>
{
    use std::{process::Stdio, thread};
    let mut child = crate::network::start_powershell(script, vars, Stdio::null()).ok_or("spawn_failed")?;
    let stdout = child.stdout.take().expect("piped quota response");
    thread::scope(|scope| {
        let reader = scope.spawn(move || serde_json::from_reader(BufReader::new(stdout)));
        let deadline = Instant::now() + timeout;
        loop
        {
            match child.try_wait()
            {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) =>
                {
                    return Err(format!("exit code={}", status.code().unwrap_or(-1)));
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                state =>
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(match state
                    {
                        Err(error) => format!("wait_failed os_code={}", error.raw_os_error().unwrap_or(-1)),
                        _ => format!("timeout limit_ms={}", timeout.as_millis()),
                    });
                }
            }
        }
        reader.join().expect("quota response reader").map_err(|err| {
            format!("invalid_json line={} column={}", err.line(), err.column())
        })
    })
}

#[cfg(not(windows))]
fn windows_quota(_: &Path, _: ToolSelection, _: &BankedResetLog) -> Providers { Providers::default() }

#[cfg(test)]
mod tests
{
    use super::*;
    use serde_json::json;

    #[test]
    fn banked_reset_preserves_zero_and_sorts_only_future_available_expirations()
    {
        let now = reset_time(&json!("2026-11-20T00:00:00Z")).unwrap();
        let bank = parse_banked_reset(&json!({"available_count": 2, "credits": [
            {"status": "available", "expires_at": "2026-12-04T12:00:00Z"},
            {"status": "redeemed", "expires_at": "2026-11-29T12:00:00Z"},
            {"status": "available", "expires_at": "2026-11-25T12:00:00Z"},
            {"status": "available", "expires_at": "2026-11-01T12:00:00Z"}
        ]}), now);
        let quota = add_banked_reset(None, bank, now).unwrap();
        let reset = quota.banked_reset.unwrap();
        assert_eq!(reset.available_count, 2);
        assert_eq!(reset.expirations, ["2026-11-25T12:00:00Z", "2026-12-04T12:00:00Z"]
            .map(|date| reset_time(&json!(date)).unwrap()));
        assert_eq!(parse_banked_reset(&json!({"available_count":0}), now).unwrap().available_count, 0);
        assert!(parse_banked_reset(&json!({}), now).is_none());
    }

    #[test]
    fn cached_limits_are_readable_during_refresh_and_preserve_failed_sources()
    {
        let log = std::env::temp_dir().join(format!("token-monitor-banked-cache-{}.log", std::process::id()));
        fs::write(&log, vec![b'x'; 128 * 1024]).unwrap();
        let cache = QuotaCache { banked_log: BankedResetLog { path: Some(log.clone()) }, ..QuotaCache::default() };
        let settings = QuotaSettings::default();
        assert!(cache.cached(settings.tools).is_none());
        let codex = |used| parse_codex(&json!({
            "type":"event_msg","timestamp":"2026-11-20T00:00:00Z","payload":{"type":"token_count",
                "rate_limits":{"primary":{"used_percent":used,"window_minutes":10080}}
            }
        }));
        cache.refresh(settings, true, || (codex(25), Providers {
            codex_reset: json!({"available_count":2}),
            deepseek: json!({"balance_infos":[{"currency":"USD","total_balance":"5"}]}),
            ..Providers::default()
        }, vec![]));
        let refreshed = cache.refresh(settings, true, || {
            assert!(cache.snapshot.try_lock().is_ok());
            assert_eq!(cache.cached(settings.tools).unwrap().tools[0].windows[0].remaining, 75.0);
            (codex(30), Providers::default(), vec![])
        });
        assert_eq!(refreshed.tools[0].windows[0].remaining, 70.0);
        assert_eq!(refreshed.tools[0].banked_reset.as_ref().unwrap().available_count, 2);
        assert_eq!(refreshed.tools[1].balances[0].amount, 5.0);
        let refreshed = cache.refresh(settings, true, || (None, Providers {
            codex_reset: json!({"available_count":0}),
            ..Providers::default()
        }, vec![]));
        assert_eq!(refreshed.tools[0].windows[0].remaining, 70.0);
        assert_eq!(refreshed.tools[0].banked_reset.as_ref().unwrap().available_count, 0);
        let without_codex = QuotaSettings { tools: ToolSelection { codex: false, ..settings.tools }, ..settings };
        let just_balance = cache.refresh(without_codex, false, || (None, Providers::default(), vec![]));
        assert!(just_balance.checked_at.is_some());
        assert_eq!(just_balance.tools.iter().map(|tool| tool.tool).collect::<Vec<_>>(), ["dsh"]);
        assert_eq!(cache.cached(settings.tools).unwrap().tools.len(), 2);
        assert_eq!(cache.refresh(without_codex, false, || panic!("fresh selection was rescanned")).tools.len(), 1);
        let trace = fs::read_to_string(&log).unwrap();
        fs::remove_file(log).unwrap();
        assert!(!trace.contains("xxxx"));
        for expected in ["source=response count=2", "source=previous count=2", "source=response count=0",
            "codex_enabled=false", "result=cache_hit"]
        {
            assert!(trace.contains(expected));
        }
    }

    #[test]
    fn quota_windows_keep_zero_and_hide_unavailable_amounts()
    {
        let codex = parse_codex(&json!({
            "type":"event_msg", "timestamp":"2026-11-20T00:00:00Z",
            "payload":{"type":"token_count","rate_limits":{
                "limit_id":"codex", "primary":{"used_percent":100,"window_minutes":10080,"resets_at":1795219200},
                "secondary":null,"plan_type":"prolite"
            }}
        })).unwrap();
        assert_eq!(codex.plan.as_deref(), Some("Pro 100"));
        assert_eq!(codex.windows[0].remaining, 0.0);
        assert_eq!(codex.windows[0].resets_at, Some(1_795_219_200_000));
        let ag = antigravity_quota(&json!({"response":{"groups":[{"displayName":"Gemini 4 Argon","buckets":[
                {"window":"WEEKLY","remaining":{"case":"remainingFraction","value":0.51},"resetTime":"2026-11-27T00:00:00Z"},
                {"window":"SESSION","remainingFraction":1.0,"disabled":true},
                {"window":"SESSION"}
            ]}]}}), &Value::Null, 0).unwrap();
        assert_eq!(ag.windows.len(), 1);
        assert_eq!(ag.windows[0].minutes, Some(10_080));
        assert_eq!(ag.windows[0].remaining, 51.0);
        let dsh = deepseek_quota(&json!({"balance_infos":[{"currency":"CNY","total_balance":"0.00"}]}), 0).unwrap();
        assert_eq!(dsh.balances[0].amount, 0.0);
    }
}
