use std::{collections::HashMap, path::PathBuf, time::Duration};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use serde::Deserialize;

use crate::{
    model::{output_rate, tool_for, Breakdown, DailyUsage, Period, Session, SessionMetrics, SessionModel, SessionPage, Summary, TokenCounts, Usage, PAGE_SIZE},
    sources::codex_index_id,
};

const CREATE_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS snapshots (
        period TEXT PRIMARY KEY, range_key TEXT NOT NULL, updated_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS usage_entries (
        period TEXT NOT NULL, source TEXT NOT NULL, tool TEXT NOT NULL, session TEXT NOT NULL,
        title TEXT NOT NULL, model TEXT NOT NULL, last_used_at INTEGER,
        input INTEGER, output INTEGER, cache_read INTEGER, cache_write INTEGER, reasoning INTEGER,
        cost REAL, missing_price INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS usage_by_session ON usage_entries(period, tool, session);
    CREATE TABLE IF NOT EXISTS session_metrics (
        period TEXT NOT NULL, tool TEXT NOT NULL, session TEXT NOT NULL,
        started_at INTEGER, workspace TEXT, calls INTEGER, timed_output INTEGER NOT NULL,
        timed_ms INTEGER NOT NULL, subagents INTEGER NOT NULL,
        background_review INTEGER NOT NULL,
        PRIMARY KEY(period, tool, session)
    );
    CREATE TABLE IF NOT EXISTS activity_days (date TEXT PRIMARY KEY, tokens INTEGER NOT NULL);
";
const SUM_USAGE: &str = "SUM(input), SUM(output), SUM(cache_read), SUM(cache_write),
                        SUM(reasoning), SUM(cost), COALESCE(MAX(missing_price), 0)";
const SESSION_SELECT: &str = "
    SELECT tool, session, MAX(title) AS title, GROUP_CONCAT(DISTINCT model) AS models,
           MAX(last_used_at) AS last_used_at, GROUP_CONCAT(DISTINCT source) AS sources,
";

#[derive(Deserialize)]
pub struct Report
{
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub sessions: Vec<SessionMeta>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry
{
    #[serde(rename = "client")]
    pub source: String,
    pub model: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cost: f64,
    pub workspace_label: Option<String>,
    pub message_count: Option<u64>,
    #[serde(default)]
    performance: Performance,
    #[serde(flatten)]
    pub counts: TokenCounts,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Performance
{
    total_duration_ms: u64,
    timed_output_tokens: u64,
    timed_reasoning_tokens: u64,
}

#[derive(Default)]
struct Metrics
{
    started_at: Option<i64>,
    workspace: Option<String>,
    calls: Option<u64>,
    timed_output: u64,
    timed_ms: u64,
    parent: Option<String>,
    background_review: bool,
}

impl Metrics
{
    fn add(&mut self, entry: &Entry, meta: Option<&SessionMeta>)
    {
        if let Some(start) = meta.and_then(|row| row.first_active_ms).filter(|time| *time > 0)
        {
            self.started_at = Some(self.started_at.map_or(start, |previous| previous.min(start)));
        }
        if self.parent.is_none() { self.parent = meta.and_then(|row| row.parent_session_id.clone()); }
        if self.workspace.is_none() { self.workspace = entry.workspace_label.clone(); }
        self.background_review |= entry.source == "codex" && entry.model == "codex-auto-review";
        if let Some(count) = entry.message_count { self.calls = Some(self.calls.unwrap_or(0) + count); }
        let performance = &entry.performance;
        // Match the original's timed generated subtotal, including disjoint reasoning.
        let output = entry.counts.output.unwrap_or(0) + entry.counts.reasoning.unwrap_or(0);
        if performance.total_duration_ms > 0
        {
            self.timed_output += (performance.timed_output_tokens + performance.timed_reasoning_tokens).min(output);
            self.timed_ms += performance.total_duration_ms;
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta
{
    #[serde(rename = "client")]
    pub source: String,
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub last_active_ms: Option<i64>,
    pub first_active_ms: Option<i64>,
    pub parent_session_id: Option<String>,
}

#[derive(Clone)]
pub struct Cache
{
    file: PathBuf,
}

impl Cache
{
    pub fn new(file: PathBuf) -> rusqlite::Result<Self>
    {
        let cache = Self { file };
        let connection = cache.connect(false)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(CREATE_SCHEMA)?;
        Ok(cache)
    }

    fn connect(&self, read_only: bool) -> rusqlite::Result<Connection>
    {
        let connection = if read_only
        {
            Connection::open_with_flags(&self.file, OpenFlags::SQLITE_OPEN_READ_ONLY)?
        }
        else
        {
            Connection::open(&self.file)?
        };
        connection.busy_timeout(Duration::from_secs(2))?;
        Ok(connection)
    }

    pub fn replace(
        &self, period: Period, range_key: &str, updated_at: i64, report: Report,
        price: impl Fn(&Entry) -> (Option<f64>, bool),
    ) -> rusqlite::Result<()>
    {
        let mut connection = self.connect(false)?;
        replace_rows(&mut connection, period, range_key, updated_at, report, price)
    }

    pub fn summary(&self, period: Period, range_key: &str) -> rusqlite::Result<Summary>
    {
        let mut connection = self.connect(true)?;
        let transaction = connection.transaction()?;
        summary(&transaction, period, range_key)
    }

    pub fn activity(&self, range_key: &str) -> rusqlite::Result<Option<Vec<DailyUsage>>>
    {
        let mut connection = self.connect(true)?;
        let tx = connection.transaction()?;
        if tx.query_row("SELECT 1 FROM snapshots WHERE period = 'activity' AND range_key = ?1",
            [range_key], |row| row.get::<_, i32>(0)).optional()?.is_none()
        {
            return Ok(None);
        }
        let mut stmt = tx.prepare("SELECT date, tokens FROM activity_days ORDER BY date")?;
        let rows = stmt.query_map([], |row| Ok(DailyUsage { date: row.get(0)?, tokens: row.get(1)? }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map(Some)
    }

    pub fn replace_activity(&self, range_key: &str, now: i64, days: &[DailyUsage]) -> rusqlite::Result<()>
    {
        let mut connection = self.connect(false)?;
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM activity_days", [])?;
        {
            let mut stmt = tx.prepare("INSERT INTO activity_days VALUES (?1, ?2)")?;
            for day in days { stmt.execute(params![day.date, day.tokens])?; }
        }
        tx.execute("INSERT OR REPLACE INTO snapshots VALUES ('activity', ?1, ?2)", params![range_key, now])?;
        tx.commit()
    }

    pub fn sessions(
        &self, period: Period, range_key: &str, tool: Option<&str>, query: &str, page: u32, limit: u32,
    ) -> rusqlite::Result<SessionPage>
    {
        let mut connection = self.connect(true)?;
        let transaction = connection.transaction()?;
        read_sessions(&transaction, period, range_key, tool, query, page, limit)
    }

    pub fn session(
        &self, period: Period, range_key: &str, tool: &str, id: &str,
    ) -> rusqlite::Result<Option<Session>>
    {
        let mut connection = self.connect(true)?;
        let connection = connection.transaction()?;
        if snapshot_time(&connection, period, range_key)?.is_none()
        {
            return Ok(None);
        }
        let mut row = connection.query_row(
            &format!("{SESSION_SELECT} {SUM_USAGE} FROM usage_entries
                      WHERE period = ?1 AND tool = ?2 AND session = ?3 GROUP BY tool, session"),
            params![period.key(), tool, id], read_session,
        ).optional()?;
        if let Some(row) = &mut row
        {
            read_metrics(&connection, period, std::slice::from_mut(row))?;
        }
        Ok(row)
    }
}

fn replace_rows(
    connection: &mut Connection, period: Period, range_key: &str, updated_at: i64, report: Report,
    price: impl Fn(&Entry) -> (Option<f64>, bool),
) -> rusqlite::Result<()>
{
    let mut metadata = HashMap::new();
    for session in &report.sessions
    {
        metadata.insert((session.source.as_str(), session.session_id.as_str()), session);
    }
    let tx = connection.transaction()?;
    tx.execute("DELETE FROM usage_entries WHERE period = ?1", [period.key()])?;
    tx.execute("DELETE FROM session_metrics WHERE period = ?1", [period.key()])?;
    let mut metrics = HashMap::<(&str, &str), Metrics>::new();
    {
        let mut statement = tx.prepare_cached(
            "INSERT INTO usage_entries VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
        )?;
        for entry in &report.entries
        {
            let Some(tool) = tool_for(&entry.source) else { continue };
            let id = entry.session_id.as_deref().unwrap_or("");
            let meta = metadata.get(&(entry.source.as_str(), id)).copied();
            let title = meta.and_then(|item| item.title.as_deref()).unwrap_or("").trim();
            let last_used = meta.and_then(|item| item.last_active_ms).filter(|millis| *millis > 0);
            let (cost, missing_price) = price(entry);
            let tokens = entry.counts;
            statement.execute(params![
                period.key(), entry.source, tool, id, title, entry.model, last_used,
                tokens.input, tokens.output, tokens.cache_read, tokens.cache_write, tokens.reasoning,
                cost, missing_price,
            ])?;
            if !id.is_empty() { metrics.entry((tool, id)).or_default().add(entry, meta); }
        }
    }
    {
        let mut subagents = HashMap::<(&str, &str), u64>::new();
        for (&(tool, id), item) in &metrics
        {
            let Some(parent) = item.parent.as_deref() else { continue };
            let parent = session_ref(tool, parent);
            if !item.background_review && parent != session_ref(tool, id)
            {
                *subagents.entry((tool, parent)).or_default() += 1;
            }
        }
        let mut statement = tx.prepare_cached(
            "INSERT INTO session_metrics VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        )?;
        for (&(tool, id), item) in &metrics
        {
            statement.execute(params![
                period.key(), tool, id, item.started_at, item.workspace, item.calls,
                item.timed_output, item.timed_ms,
                subagents.get(&(tool, session_ref(tool, id))).copied().unwrap_or(0), item.background_review,
            ])?;
        }
    }
    tx.execute("INSERT OR REPLACE INTO snapshots VALUES (?1, ?2, ?3)", params![period.key(), range_key, updated_at])?;
    tx.commit()
}

fn session_ref<'a>(tool: &str, id: &'a str) -> &'a str
{
    if tool == "codex" { codex_index_id(id).unwrap_or(id) } else { id }
}

fn snapshot_time(connection: &Connection, period: Period, range_key: &str) -> rusqlite::Result<Option<i64>>
{
    connection.query_row(
        "SELECT updated_at FROM snapshots WHERE period = ?1 AND range_key = ?2",
        params![period.key(), range_key], |row| row.get(0),
    ).optional()
}

fn record_usage(row: &Row<'_>, column: usize) -> rusqlite::Result<Usage>
{
    let counts = TokenCounts {
        input: row.get(column)?,
        output: row.get(column + 1)?,
        cache_read: row.get(column + 2)?,
        cache_write: row.get(column + 3)?,
        reasoning: row.get(column + 4)?,
    };
    Ok(Usage::new(counts, row.get(column + 5)?, row.get(column + 6)?))
}

fn summary(connection: &Connection, period: Period, range_key: &str) -> rusqlite::Result<Summary>
{
    let updated_at = snapshot_time(connection, period, range_key)?;
    let mut summary = Summary { updated_at, ..Summary::default() };
    if updated_at.is_none()
    {
        return Ok(summary);
    }
    let mut grouped = connection.prepare(&format!(
        "SELECT tool, model, {SUM_USAGE} FROM usage_entries WHERE period = ?1 GROUP BY tool, model"
    ))?;
    let items = grouped.query_map([period.key()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, record_usage(row, 2)?))
    })?;
    let mut tools = HashMap::<String, Breakdown>::new();
    let mut models = HashMap::<String, Breakdown>::new();
    summary.usage.cost_usd = None;
    for entry in items
    {
        let (tool, model, usage) = entry?;
        summary.usage.add(usage);
        let tool = tools.entry(tool.clone()).or_insert_with(|| Breakdown::new(tool, Usage::default()));
        tool.usage.add(usage);
        tool.models.push(Breakdown::new(model.clone(), usage));
        let model = models.entry(model.clone()).or_insert_with(|| Breakdown::new(model, Usage::default()));
        model.usage.add(usage);
    }
    if summary.usage.tokens == 0 && !summary.usage.missing_price
    {
        summary.usage.cost_usd = Some(0.0);
    }
    summary.tools = tools.into_values().collect();
    summary.models = models.into_values().collect();
    rank(&mut summary.tools);
    rank(&mut summary.models);
    for tool in &mut summary.tools
    {
        rank(&mut tool.models);
    }
    Ok(summary)
}

fn rank(items: &mut [Breakdown])
{
    items.sort_by(|first, second| second.usage.tokens.cmp(&first.usage.tokens).then_with(|| first.key.cmp(&second.key)));
}

fn split_names(value: String) -> Vec<String>
{
    let mut names: Vec<String> = value.split(',').filter(|value| !value.is_empty()).map(str::to_owned).collect();
    names.sort();
    names
}

fn read_session(row: &Row<'_>) -> rusqlite::Result<Session>
{
    Ok(Session {
        tool: row.get(0)?,
        id: row.get(1)?,
        title: row.get(2)?,
        models: split_names(row.get(3)?),
        last_used_at: row.get(4)?,
        sources: split_names(row.get(5)?),
        usage: record_usage(row, 6)?,
        metrics: SessionMetrics::default(),
        model_usage: Vec::new(),
        native_id: None,
        reasoning_effort: None,
        latest_model: None,
        archived: false,
        activity: None,
        context: None,
    })
}

fn read_metrics(connection: &Connection, period: Period, rows: &mut [Session]) -> rusqlite::Result<()>
{
    let mut metrics = connection.prepare(
        "SELECT started_at, workspace, calls, timed_output, timed_ms, subagents, background_review
         FROM session_metrics WHERE period = ?1 AND tool = ?2 AND session = ?3",
    )?;
    let mut models = connection.prepare(
        "SELECT model, SUM(COALESCE(input,0)+COALESCE(output,0)+COALESCE(cache_read,0)
                    +COALESCE(cache_write,0)+COALESCE(reasoning,0)) AS tokens
         FROM usage_entries WHERE period = ?1 AND tool = ?2 AND session = ?3
         GROUP BY model HAVING tokens > 0 ORDER BY tokens DESC, model",
    )?;
    for row in rows
    {
        let key = params![period.key(), row.tool, row.id];
        row.metrics = metrics.query_row(key, |record| Ok(SessionMetrics {
            started_at: record.get(0)?,
            workspace: record.get(1)?,
            calls: record.get(2)?,
            tokens_per_second: output_rate(record.get(3)?, record.get(4)?),
            subagents: record.get(5)?,
            background_review: record.get(6)?,
        })).optional()?.unwrap_or_default();
        if row.models.len() > 1
        {
            row.model_usage = models.query_map(key, |record| Ok(SessionModel {
                model: record.get(0)?,
                tokens: record.get(1)?,
            }))?.collect::<rusqlite::Result<_>>()?;
        }
    }
    Ok(())
}

fn read_sessions(
    connection: &Connection, period: Period, range_key: &str, tool: Option<&str>, query: &str, page: u32, limit: u32,
) -> rusqlite::Result<SessionPage>
{
    if !period.has_sessions() || snapshot_time(connection, period, range_key)?.is_none()
    {
        return Ok(SessionPage::default());
    }
    let mut list_tools = connection.prepare(
        "SELECT DISTINCT tool FROM usage_entries WHERE period = ? AND session <> '' ORDER BY tool"
    )?;
    let tools = list_tools.query_map([period.key()], |row| row.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    let filter = tool.filter(|tool| tools.iter().any(|value| value == *tool));
    let query = query.trim();
    let filtered = "FROM usage_entries WHERE period = ?1 AND session <> '' AND (?2 IS NULL OR tool = ?2)
        GROUP BY tool, session HAVING ?3 = '' OR instr(lower(MAX(title)), lower(?3)) > 0
        OR instr(lower(GROUP_CONCAT(DISTINCT model)), lower(?3)) > 0 OR instr(lower(tool), lower(?3)) > 0
        OR instr(lower(session), lower(?3)) > 0";
    let total = connection.query_row(
        &format!("SELECT COUNT(*) FROM (SELECT 1 {filtered})"), params![period.key(), filter, query], |row| row.get(0),
    )?;
    let mut statement = connection.prepare(&format!(
        "{SESSION_SELECT} {SUM_USAGE} {filtered} ORDER BY last_used_at DESC, tool, session LIMIT ?4 OFFSET ?5"
    ))?;
    let mut rows = statement.query_map(params![period.key(), filter, query, limit, u64::from(page) * u64::from(PAGE_SIZE)], read_session)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    read_metrics(connection, period, &mut rows)?;
    Ok(SessionPage { rows, total, tools })
}

#[cfg(test)]
mod tests
{
    use super::*;
    use serde_json::json;

    #[test]
    fn session_metrics_use_only_timed_generated_tokens()
    {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(CREATE_SCHEMA).unwrap();
        let report = serde_json::from_value(json!({
            "entries": [
                {"client":"codex","model":"a","sessionId":"main","input":30,"output":6,"reasoning":4,"cacheRead":10,
                    "messageCount":2,"workspaceLabel":"Project","performance":{
                        "totalDurationMs":2000,"timedOutputTokens":6,"timedReasoningTokens":4
                    }},
                {"client":"codex","model":"b","sessionId":"main","input":40,"output":6,"reasoning":4,"messageCount":1},
                {"client":"codex","model":"a","sessionId":"child","input":11},
                {"client":"codex","model":"codex-auto-review","sessionId":"review","input":12}
            ],
            "sessions": [
                {"client":"codex","sessionId":"main","firstActiveMs":30,"lastActiveMs":100},
                {"client":"codex","sessionId":"child","parentSessionId":"main"},
                {"client":"codex","sessionId":"review","parentSessionId":"main"}
            ]
        })).unwrap();
        replace_rows(&mut db, Period::Today, "today", 120, report, |_| (None, true)).unwrap();
        let preview = read_sessions(&db, Period::Today, "today", None, "", 0, 1).unwrap();
        assert_eq!((preview.rows.len(), preview.total), (1, 3));
        let main = &preview.rows[0];
        assert_eq!(main.usage.tokens, 100);
        assert_eq!(main.metrics.started_at, Some(30));
        assert_eq!(main.metrics.calls, Some(3));
        assert_eq!(main.metrics.tokens_per_second, Some(5.0));
        assert_eq!(main.metrics.workspace.as_deref(), Some("Project"));
        assert_eq!(main.metrics.subagents, 1);
        assert_eq!(main.model_usage.iter().map(|model| model.tokens).collect::<Vec<_>>(), [50, 50]);
        let review = read_sessions(&db, Period::Today, "today", None, "review", 0, PAGE_SIZE).unwrap();
        assert!(review.rows[0].metrics.background_review);
        assert_eq!(review.rows[0].metrics.tokens_per_second, None);
    }

    #[test]
    fn replacement_removes_deleted_sessions_and_keeps_unpriced_usage()
    {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(CREATE_SCHEMA).unwrap();
        let make_report = || serde_json::from_value(json!({
            "entries": [
                {"client":"antigravity-cli","model":"a","sessionId":"one","input":8,"reasoning":2,"cost":0.125},
                {"client":"antigravity-extension","model":"b","sessionId":"two","input":5},
                {"client":"codex","model":"other","sessionId":"three","input":7}
            ],
            "sessions": [{"client":"antigravity-cli","sessionId":"one","title":"A project","lastActiveMs":65}]
        })).unwrap();
        let price = |row: &Entry| if row.model == "a" { (Some(row.cost), false) } else { (None, true) };
        replace_rows(&mut db, Period::Today, "today", 100, make_report(), price).unwrap();
        let result = summary(&db, Period::Today, "today").unwrap();
        assert_eq!(result.usage.tokens, 22);
        assert_eq!(result.usage.cost_usd, Some(0.125));
        assert!(result.usage.missing_price);
        assert_eq!(result.tools[0].key, "antigravity");
        assert_eq!(result.tools[0].models.len(), 2);
        let page = read_sessions(&db, Period::Today, "today", Some("antigravity"), "PROJECT", 0, PAGE_SIZE).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].id, "one");
        let mut invalid = make_report();
        invalid.entries[1].counts.input = Some(u64::MAX);
        assert!(replace_rows(&mut db, Period::Today, "today", 150, invalid, price).is_err());
        assert_eq!(summary(&db, Period::Today, "today").unwrap().usage.tokens, 22);
        let mut replacement = make_report();
        replacement.entries.remove(0);
        replace_rows(&mut db, Period::Today, "today", 200, replacement, price).unwrap();
        assert_eq!(summary(&db, Period::Today, "today").unwrap().usage.tokens, 12);
        assert_eq!(read_sessions(&db, Period::Today, "today", Some("antigravity"), "PROJECT", 0, PAGE_SIZE).unwrap().total, 0);
        assert_eq!(summary(&db, Period::Today, "tomorrow").unwrap().updated_at, None);
    }
}
