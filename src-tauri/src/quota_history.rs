use std::{path::{Path, PathBuf}, time::Duration};

use chrono::{DateTime, Days, Utc};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::model::system_zone;

const DAY_MS: i64 = 86_400_000;
const DAYS: usize = 14;
pub const RETENTION_MS: i64 = 15 * DAY_MS;
const RESET_TOLERANCE: u64 = 1_000;
const TREND_LIMIT: usize = 180;
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS readings (
        series TEXT NOT NULL, at INTEGER NOT NULL, value REAL NOT NULL, reset_at INTEGER,
        PRIMARY KEY(series, at)
    ) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS readings_by_time ON readings(at);
    CREATE TABLE IF NOT EXISTS quota_profiles (
        tool TEXT NOT NULL, at INTEGER NOT NULL, plan TEXT, account TEXT, PRIMARY KEY(tool, at)
    ) WITHOUT ROWID;
";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QuotaKey
{
    Window { tool: String, group: String, minutes: Option<u64> },
    Balance { tool: String, currency: String },
}

impl QuotaKey
{
    pub fn tool(&self) -> &str
    {
        match self
        {
            Self::Window { tool, .. } | Self::Balance { tool, .. } => tool,
        }
    }

    fn minutes(&self) -> Option<u64>
    {
        match self
        {
            Self::Window { minutes, .. } => *minutes,
            Self::Balance { .. } => None,
        }
    }

    pub(crate) fn identity(&self) -> String
    {
        // JSON keeps provider, model group, duration and currency unambiguous.
        serde_json::to_string(self).expect("quota identity")
    }
}

#[derive(Clone, Copy)]
pub struct Reading
{
    pub at: i64,
    pub value: f64,
    pub reset_at: Option<i64>,
}

pub struct Sample
{
    pub key: QuotaKey,
    pub reading: Reading,
    pub plan: Option<String>,
    pub account: Option<String>,
}

pub struct QuotaHistory
{
    file: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPreview
{
    start: i64,
    end: i64,
    cycle: bool,
    observed_at: Option<i64>,
    trend: Vec<TrendPoint>,
    days: Vec<DailyUsage>,
    unallocated: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrendPoint
{
    at: i64,
    value: f64,
    split: bool,
}

#[derive(Serialize)]
struct DailyUsage
{
    date: String,
    used: Option<f64>,
}

impl QuotaHistory
{
    pub fn new(root: &Path) -> Self { Self { file: root.join("quota-history.sqlite") } }

    pub(crate) fn connection(&self) -> rusqlite::Result<Connection>
    {
        let db = Connection::open(&self.file)?;
        db.busy_timeout(Duration::from_secs(2))?;
        db.execute_batch(SCHEMA)?;
        Ok(db)
    }

    pub fn record(&self, samples: &[Sample], now: i64) -> rusqlite::Result<()>
    {
        if samples.is_empty() { return Ok(()); }
        record(&mut self.connection()?, samples, now)
    }

    pub fn preview(&self, key: &QuotaKey) -> rusqlite::Result<HistoryPreview>
    {
        let now = Utc::now();
        let readings = if self.file.is_file()
        {
            let db = Connection::open_with_flags(&self.file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            db.busy_timeout(Duration::from_millis(750))?;
            read(&db, key, now.timestamp_millis())?
        }
        else { vec![] };
        Ok(preview(key, &readings, now, system_zone()))
    }
}

pub(crate) fn store_samples(db: &Connection, samples: &[Sample], now: i64) -> rusqlite::Result<()>
{
    let cutoff = now - RETENTION_MS;
    let mut insert = db.prepare_cached("INSERT OR IGNORE INTO readings VALUES (?1, ?2, ?3, ?4)")?;
    for sample in samples
    {
        let reading = sample.reading;
        if reading.at >= cutoff && reading.at <= now && reading.value.is_finite()
        {
            insert.execute(params![sample.key.identity(), reading.at, reading.value, reading.reset_at])?;
            if sample.plan.is_some() || sample.account.is_some()
            {
                store_profile(db, sample.key.tool(), reading.at, sample.plan.as_deref(), sample.account.as_deref())?;
            }
        }
    }
    Ok(())
}

pub(crate) fn store_profile(db: &Connection, tool: &str, at: i64, plan: Option<&str>, account: Option<&str>) -> rusqlite::Result<()>
{
    db.prepare_cached("INSERT INTO quota_profiles VALUES (?1,?2,?3,?4)
        ON CONFLICT(tool,at) DO UPDATE SET plan=COALESCE(excluded.plan,plan), account=COALESCE(excluded.account,account)")?
        .execute(params![tool, at, plan, account])?;
    Ok(())
}

fn record(db: &mut Connection, samples: &[Sample], now: i64) -> rusqlite::Result<()>
{
    let tx = db.transaction()?;
    store_samples(&tx, samples, now)?;
    tx.execute("DELETE FROM readings WHERE at < ?1", [now - RETENTION_MS])?;
    tx.execute("DELETE FROM quota_profiles WHERE at < ?1", [now - RETENTION_MS])?;
    tx.commit()
}

pub(crate) fn read(db: &Connection, key: &QuotaKey, now: i64) -> rusqlite::Result<Vec<Reading>>
{
    let mut query = db.prepare("SELECT at, value, reset_at FROM readings
        WHERE series = ?1 AND at BETWEEN ?2 AND ?3 ORDER BY at")?;
    let result = query.query_map(params![key.identity(), now - RETENTION_MS, now], |row| Ok(Reading {
        at: row.get(0)?, value: row.get(1)?, reset_at: row.get(2)?,
    }))?.collect();
    result
}

pub(crate) fn same_reset(left: i64, right: i64) -> bool { left.abs_diff(right) <= RESET_TOLERANCE }

fn consumption(previous: Reading, current: Reading) -> Option<f64>
{
    if current.value > previous.value { return None; }
    let compatible = match (previous.reset_at, current.reset_at)
    {
        (None, None) => true,
        (Some(a), Some(b)) => same_reset(a, b) && current.at < a.min(b),
        _ => false,
    };
    compatible.then_some(previous.value - current.value)
}

fn preview(key: &QuotaKey, readings: &[Reading], now: DateTime<Utc>, zone: Tz) -> HistoryPreview
{
    let now_ms = now.timestamp_millis();
    let today = now.with_timezone(&zone).date_naive();
    let first = today - Days::new((DAYS - 1) as u64);
    let mut days: Vec<_> = (0..DAYS).map(|i| DailyUsage { date: (first + Days::new(i as u64)).to_string(), used: None }).collect();
    let date_of = |at| DateTime::from_timestamp_millis(at).expect("retained timestamp").with_timezone(&zone).date_naive();
    let mut unallocated = 0.0;
    for pair in readings.windows(2)
    {
        let Some(used) = consumption(pair[0], pair[1]) else { continue };
        let date = date_of(pair[1].at);
        let index = (date - first).num_days();
        if date_of(pair[0].at) == date
        {
            if let Some(day) = days.get_mut(index as usize)
            {
                *day.used.get_or_insert(0.0) += used;
            }
        }
        else if index >= 0
        {
            // A gap across midnight gives a total without identifying the day.
            unallocated += used;
        }
    }
    let latest = readings.last();
    let cycle = latest.and_then(|point| {
        let reset = point.reset_at?;
        let duration = i64::try_from(key.minutes()?).ok()?.checked_mul(60_000)?;
        let start = reset.checked_sub(duration)?;
        (start >= 0 && point.at >= start && point.at < reset).then_some((start, reset))
    });
    let (start, end) = cycle.unwrap_or((now_ms - (DAYS as i64) * DAY_MS, now_ms));
    let visible: Vec<_> = readings.iter().copied().filter(|point| {
        point.at >= start && point.at <= end &&
            (cycle.is_none() || point.reset_at.is_some_and(|reset| same_reset(reset, end)))
    }).collect();
    let step = visible.len().div_ceil(TREND_LIMIT).max(1);
    let mut trend = Vec::new();
    let mut split = true;
    for (index, point) in visible.iter().copied().enumerate()
    {
        split |= index > 0 && consumption(visible[index - 1], point).is_none();
        if index % step == 0 || index + 1 == visible.len()
        {
            trend.push(TrendPoint { at: point.at, value: point.value, split });
            split = false;
        }
    }
    HistoryPreview { start, end, cycle: cycle.is_some(), observed_at: latest.map(|row| row.at), trend, days, unallocated }
}

#[cfg(test)]
mod tests
{
    use super::*;

    fn window(group: &str) -> QuotaKey
    {
        QuotaKey::Window { tool: "antigravity".into(), group: group.into(), minutes: Some(10_080) }
    }

    #[test]
    fn usage_ignores_resets_refills_and_cross_midnight_attribution()
    {
        let now = "2026-11-20T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let time = now.timestamp_millis();
        let reset = time + DAY_MS;
        let sample = |hours: i64, value, reset_at| Reading { at: time - hours * 3_600_000, value, reset_at: Some(reset_at) };
        let recordings = [
            sample(14, 95.0, reset),
            sample(11, 90.0, reset),
            sample(10, 85.0, reset + 500),
            sample(9, 100.0, reset + DAY_MS),
            sample(8, 98.0, reset + DAY_MS),
            sample(7, 99.0, reset + DAY_MS),
            sample(6, 96.0, reset + DAY_MS),
        ];
        let result = preview(&window("Gemini"), &recordings, now, chrono_tz::UTC);
        assert!(result.cycle);
        assert_eq!(result.end, reset + DAY_MS);
        assert_eq!(result.days.last().unwrap().used, Some(10.0));
        assert_eq!(result.days[DAYS - 2].used, None);
        assert_eq!(result.unallocated, 5.0);
        assert_eq!(result.trend.iter().map(|p| p.split).collect::<Vec<_>>(), [true, false, true, false]);
    }

    #[test]
    fn storage_deduplicates_and_keeps_window_histories_separate()
    {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let now = 2_000_000_000_000;
        let sample = |group, at, value| Sample { key: window(group), reading: Reading { at, value, reset_at: None }, plan: None, account: None };
        record(&mut db, &[sample("Gemini", now - 100, 71.0), sample("Argon", now - 100, 61.0)], now).unwrap();
        record(&mut db, &[sample("Gemini", now - 100, 60.0), sample("Gemini", now - 1000, 73.0),
            sample("Gemini", now - RETENTION_MS - 1, 100.0)], now).unwrap();
        let gemini = read(&db, &window("Gemini"), now).unwrap();
        assert_eq!(gemini.iter().map(|p| p.value).collect::<Vec<_>>(), [73.0, 71.0]);
        assert_eq!(read(&db, &window("Argon"), now).unwrap()[0].value, 61.0);
    }
}
