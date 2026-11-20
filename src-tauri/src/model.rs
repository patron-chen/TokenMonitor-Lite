use chrono::{DateTime, Datelike, Days, LocalResult, NaiveDate, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Period
{
    Today,
    Month,
    Week,
    Last7,
    Last30,
    AllTime,
}

impl Period
{
    pub fn key(self) -> &'static str
    {
        match self
        {
            Self::Today => "today",
            Self::Month => "month",
            Self::Week => "week",
            Self::Last7 => "last7",
            Self::Last30 => "last30",
            Self::AllTime => "allTime",
        }
    }

    pub fn has_sessions(self) -> bool
    {
        matches!(self, Self::Today | Self::Month | Self::AllTime)
    }
}

pub fn system_zone() -> Tz
{
    iana_time_zone::get_timezone().ok().and_then(|zone| zone.parse().ok()).unwrap_or(chrono_tz::UTC)
}

pub struct Span
{
    pub zone: Tz,
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
}

impl Span
{
    pub fn new(period: Period, now: DateTime<Utc>, zone: Tz, first_day: Weekday) -> Self
    {
        let day = now.with_timezone(&zone).date_naive();
        let (start, end) = match period
        {
            Period::Today => (Some(day), Some(day)),
            Period::Month => (day.with_day(1), None),
            Period::AllTime => (None, None),
            Period::Week =>
            {
                let offset = (day.weekday().num_days_from_monday() + 7 - first_day.num_days_from_monday()) % 7;
                (day.checked_sub_days(Days::new(u64::from(offset))), Some(day))
            }
            Period::Last7 => (day.checked_sub_days(Days::new(6)), Some(day)),
            Period::Last30 => (day.checked_sub_days(Days::new(29)), Some(day)),
        };
        Self { zone, start, end }
    }

    pub fn key(&self) -> String
    {
        format!("{}|{}|{}", self.zone, date_key(self.start), date_key(self.end))
    }

    pub fn contains(&self, millis: Option<i64>) -> bool
    {
        if self.start.is_none() && self.end.is_none()
        {
            return true;
        }
        let Some(time) = millis.and_then(DateTime::from_timestamp_millis) else { return false };
        let day = time.with_timezone(&self.zone).date_naive();
        let in_month = self.end.is_some() || self.start.is_none_or(|start| start.month() == day.month() && start.year() == day.year());
        in_month && self.start.is_none_or(|start| start <= day) && self.end.is_none_or(|end| day <= end)
    }
}

fn date_key(day: Option<NaiveDate>) -> String
{
    day.map(|day| day.to_string()).unwrap_or_default()
}

pub fn next_midnight(now: DateTime<Utc>, zone: Tz) -> DateTime<Utc>
{
    let tomorrow = now.with_timezone(&zone).date_naive().succ_opt().expect("next calendar day");
    // A transition can skip midnight. Use the first local time that exists that day.
    for hour in 0..24
    {
        let time = tomorrow.and_hms_opt(hour, 0, 0).expect("valid hour");
        match zone.from_local_datetime(&time)
        {
            LocalResult::Single(time) | LocalResult::Ambiguous(time, _) => return time.with_timezone(&Utc),
            LocalResult::None => {}
        }
    }
    now + chrono::TimeDelta::hours(24)
}

pub fn tool_for(source: &str) -> Option<&'static str>
{
    match source
    {
        "codex" => Some("codex"),
        "antigravity-cli" | "antigravity-extension" => Some("antigravity"),
        "dsh" => Some("dsh"),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize, Hash)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts
{
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
}

impl TokenCounts
{
    pub fn total(self) -> u64
    {
        [self.input, self.output, self.cache_read, self.cache_write, self.reasoning]
            .into_iter().flatten().sum()
    }

    pub fn add(&mut self, other: Self)
    {
        for (current, next) in [
            (&mut self.input, other.input),
            (&mut self.output, other.output),
            (&mut self.cache_read, other.cache_read),
            (&mut self.cache_write, other.cache_write),
            (&mut self.reasoning, other.reasoning),
        ]
        {
            if let Some(next) = next
            {
                *current = Some(current.unwrap_or(0) + next);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage
{
    pub counts: TokenCounts,
    pub tokens: u64,
    pub cost_usd: Option<f64>,
    pub missing_price: bool,
}

impl Usage
{
    pub fn new(counts: TokenCounts, cost_usd: Option<f64>, missing_price: bool) -> Self
    {
        Self { tokens: counts.total(), counts, cost_usd, missing_price }
    }

    pub fn add(&mut self, other: Self)
    {
        self.counts.add(other.counts);
        self.tokens += other.tokens;
        if let Some(price) = other.cost_usd
        {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + price);
        }
        self.missing_price |= other.missing_price;
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakdown
{
    pub key: String,
    pub usage: Usage,
    pub models: Vec<Breakdown>,
}

impl Breakdown
{
    pub fn new(key: String, usage: Usage) -> Self
    {
        Self { key, usage, models: Vec::new() }
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary
{
    pub updated_at: Option<i64>,
    pub usage: Usage,
    pub tools: Vec<Breakdown>,
    pub models: Vec<Breakdown>,
    pub refreshing: bool,
    pub failure: Option<String>,
    pub antigravity_local: bool,
    pub antigravity_pb_only: bool,
}

#[derive(Deserialize, Serialize)]
pub struct DailyUsage
{
    pub date: String,
    pub tokens: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity
{
    pub today: String,
    pub today_tokens: u64,
    pub days: Vec<DailyUsage>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session
{
    pub tool: String,
    pub id: String,
    pub title: String,
    pub models: Vec<String>,
    pub last_used_at: Option<i64>,
    pub usage: Usage,
    pub metrics: SessionMetrics,
    pub model_usage: Vec<SessionModel>,
    pub native_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub latest_model: Option<String>,
    pub archived: bool,
    pub activity: Option<SessionActivity>,
    pub context: Option<SessionContext>,
    #[serde(skip)]
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetrics
{
    pub started_at: Option<i64>,
    pub workspace: Option<String>,
    pub calls: Option<u64>,
    pub tokens_per_second: Option<f64>,
    pub subagents: u64,
    pub background_review: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionModel
{
    pub model: String,
    pub tokens: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionActivity
{
    pub state: &'static str,
    pub at: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext
{
    pub tokens: u64,
    pub window: u64,
    pub at: i64,
}

pub fn output_rate(tokens: u64, millis: u64) -> Option<f64>
{
    (tokens > 0 && millis > 0).then(|| tokens as f64 * 1000.0 / millis as f64)
}

pub fn reasoning_effort(value: &str) -> Option<String>
{
    let value = value.trim();
    let valid = value.len() <= 32 && value.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        && !value.eq_ignore_ascii_case("unknown");
    valid.then(|| value.to_owned())
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage
{
    pub rows: Vec<Session>,
    pub total: u64,
    pub tools: Vec<String>,
}

pub const PAGE_SIZE: u32 = 100;

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn periods_use_local_calendar_dates()
    {
        let now = DateTime::parse_from_rfc3339("2026-11-30T16:10:00Z").unwrap().to_utc();
        let day = Span::new(Period::Today, now, chrono_tz::Asia::Shanghai, Weekday::Mon);
        assert_eq!(day.start.unwrap().to_string(), "2026-12-01");
        assert!(day.contains(Some(now.timestamp_millis())));
        assert!(!day.contains(Some((now - chrono::TimeDelta::minutes(20)).timestamp_millis())));
        let week = Span::new(Period::Week, now, chrono_tz::Asia::Shanghai, Weekday::Mon);
        assert_eq!(week.start.unwrap().to_string(), "2026-11-30");
        let last7 = Span::new(Period::Last7, now, chrono_tz::Asia::Shanghai, Weekday::Mon);
        assert_eq!(last7.start.unwrap().to_string(), "2026-11-25");
        let month = Span::new(Period::Month, now, chrono_tz::Asia::Shanghai, Weekday::Mon);
        assert!(!month.contains(Some((now + chrono::TimeDelta::days(60)).timestamp_millis())));
    }
}
