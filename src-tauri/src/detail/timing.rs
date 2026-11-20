use std::{collections::{HashMap, HashSet}, path::Path, time::Duration};

use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;

use crate::{model::{output_rate, Session}, sources::Sources};

use super::Exchange;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TranscriptTiming
{
    elapsed_ms: u64,
    first_token_ms: Option<u64>,
}

impl TranscriptTiming
{
    pub fn from_elapsed(elapsed_ms: u64, first_token_ms: Option<u64>) -> Option<Self>
    {
        (elapsed_ms > 0).then(|| Self {
            elapsed_ms,
            first_token_ms: first_token_ms.filter(|first| *first > 0 && *first <= elapsed_ms),
        })
    }

    pub fn from_events(start: i64, end: i64, first: Option<i64>) -> Option<Self>
    {
        let elapsed_ms = u64::try_from(end.checked_sub(start)?).ok()?;
        let first_token_ms = first.and_then(|first| u64::try_from(first.checked_sub(start)?).ok());
        Self::from_elapsed(elapsed_ms, first_token_ms)
    }

    fn performance(self, generated: u64) -> Option<Performance>
    {
        let rate = (generated >= 200).then(|| output_rate(generated, self.elapsed_ms)).flatten();
        Performance::new(Source::Transcript, rate, self.first_token_ms, false)
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum Source { Routed, Transcript }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Performance
{
    source: Source,
    tokens_per_second: Option<f64>,
    first_token_ms: Option<u64>,
    hidden_reasoning: bool,
}

impl Performance
{
    fn new(source: Source, tokens_per_second: Option<f64>, first_token_ms: Option<u64>, hidden_reasoning: bool) -> Option<Self>
    {
        (tokens_per_second.is_some() || first_token_ms.is_some())
            .then_some(Self { source, tokens_per_second, first_token_ms, hidden_reasoning })
    }
}

#[derive(Clone, Copy)]
struct RouteTiming
{
    output: u64,
    elapsed_ms: u64,
    first_token_ms: Option<u64>,
}

impl RouteTiming
{
    fn performance(self, hidden_reasoning: bool) -> Option<Performance>
    {
        let first = self.first_token_ms.filter(|first| *first <= self.elapsed_ms && self.elapsed_ms > 0);
        let rate = first.filter(|_| self.output >= 100)
            .and_then(|first| output_rate(self.output, self.elapsed_ms - first));
        Performance::new(Source::Routed, rate, first, hidden_reasoning)
    }
}

pub(super) fn populate(sources: &Sources, session: &Session, exchanges: &mut [Exchange])
{
    let routes = routes_for_page(sources.cc_switch_db(), session, exchanges).unwrap_or_default();
    for turn in exchanges.iter_mut().flat_map(|exchange| &mut exchange.turns)
    {
        let routed = turn.response_id.as_deref().and_then(|id| routes.get(id));
        let generated = turn.counts.output.unwrap_or(0) + turn.counts.reasoning.unwrap_or(0);
        turn.performance = match routed
        {
            // A matched router row wins even when it has no usable clocks.
            Some(route) => route.and_then(|sample| sample.performance(!turn.reasoning_exposed && turn.counts.reasoning.unwrap_or(0) > 0)),
            None => turn.timing.and_then(|timing| timing.performance(generated)),
        };
    }
}

fn routes_for_page(path: &Path, session: &Session, exchanges: &[Exchange])
    -> rusqlite::Result<HashMap<String, Option<RouteTiming>>>
{
    let app = match session.tool.as_str()
    {
        "codex" => "codex",
        "antigravity" => "agy",
        "dsh" => "dsh",
        _ => return Ok(HashMap::new()),
    };
    let turns = exchanges.iter().flat_map(|exchange| &exchange.turns);
    let ids: HashSet<&str> = turns.clone().filter_map(|turn| turn.response_id.as_deref()).collect();
    if ids.is_empty() || !path.is_file() { return Ok(HashMap::new()); }
    let (mut earliest, mut latest) = (i64::MAX, 0_i64);
    for at in turns.filter_map(|turn| turn.at)
    {
        earliest = earliest.min(at / 1000);
        latest = latest.max(at / 1000);
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_millis(150))?;
    read_routes(&db, app, session.native_id.as_deref().unwrap_or(&session.id), earliest, latest, &ids)
}

fn read_routes(
    db: &Connection, app: &str, session: &str, earliest: i64, latest: i64, ids: &HashSet<&str>,
) -> rusqlite::Result<HashMap<String, Option<RouteTiming>>>
{
    // The router may assign its own session ID. The time range bounds that
    // lookup using its created_at index, but only an exact response ID joins.
    let mut query = db.prepare(
        "SELECT request_id, provider_id, output_tokens, latency_ms, first_token_ms
         FROM proxy_request_logs WHERE app_type = ?1 AND data_source = 'proxy'
         AND (session_id = ?2 OR created_at BETWEEN ?3 AND ?4)",
    )?;
    let mut rows = query.query(params![app, session, earliest.saturating_sub(600), latest.saturating_add(600)])?;
    let mut matched = HashMap::new();
    while let Some(row) = rows.next()?
    {
        let key: String = row.get(0)?;
        let provider: String = row.get(1)?;
        let scoped = format!("session:{app}:{provider}:");
        let id = key.strip_prefix(&scoped).or_else(|| key.strip_prefix("session:")).unwrap_or(&key);
        if !ids.contains(id) { continue; }
        let sample = RouteTiming { output: row.get(2)?, elapsed_ms: row.get(3)?, first_token_ms: row.get(4)? };
        // Provider-scoped IDs can share an upstream ID. Leave an ambiguous
        // match empty instead of attributing timing to the wrong response.
        matched.entry(id.to_owned()).and_modify(|entry| *entry = None).or_insert(Some(sample));
    }
    Ok(matched)
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn thresholds_and_first_token_use_the_selected_clock()
    {
        let local = TranscriptTiming::from_events(1000, 6000, Some(2000)).unwrap();
        let short = local.performance(199).unwrap();
        assert_eq!(short.tokens_per_second, None);
        assert_eq!(short.first_token_ms, Some(1000));
        let long = local.performance(200).unwrap();
        assert_eq!(long.tokens_per_second, Some(40.0));
        assert!(matches!(long.source, Source::Transcript));
        let route = RouteTiming { output: 100, elapsed_ms: 5000, first_token_ms: Some(1000) };
        let measured = route.performance(true).unwrap();
        assert!(matches!(measured.source, Source::Routed));
        assert_eq!(measured.tokens_per_second, Some(25.0));
        assert!(measured.hidden_reasoning);
        assert_eq!(RouteTiming { output: 99, ..route }.performance(false).unwrap().tokens_per_second, None);
        assert!(RouteTiming { first_token_ms: None, ..route }.performance(false).is_none());
    }

    #[test]
    fn joins_router_rows_by_response_id_and_ignores_imported_sessions()
    {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("
          CREATE TABLE proxy_request_logs (
            request_id TEXT, provider_id TEXT, app_type TEXT, output_tokens INTEGER, latency_ms INTEGER,
            first_token_ms INTEGER, status_code INTEGER, data_source TEXT, session_id TEXT, created_at INTEGER
          );
          INSERT INTO proxy_request_logs VALUES ('session:codex:p:resp_1','p','codex',320,5200,1200,200,'proxy','other',10000);
          INSERT INTO proxy_request_logs VALUES ('session:codex:p:resp_2','p','codex',400,5500,2100,200,'codex_session','one',10000);
          INSERT INTO proxy_request_logs VALUES ('session:codex:p:resp_3','p','codex',900,6000,NULL,200,'proxy','one',8000);
          INSERT INTO proxy_request_logs VALUES ('session:codex:p:resp_4','p','codex',100,6000,3000,200,'proxy','one',10000);
          INSERT INTO proxy_request_logs VALUES ('session:codex:q:resp_4','q','codex',100,6000,5000,200,'proxy','one',10000);
        ").unwrap();
        let ids = HashSet::from(["resp_1", "resp_2", "resp_3", "resp_4"]);
        let result = read_routes(&db, "codex", "one", 10000, 10000, &ids).unwrap();
        let route = result["resp_1"].unwrap().performance(false).unwrap();
        assert_eq!(route.tokens_per_second, Some(80.0));
        assert_eq!(route.first_token_ms, Some(1200));
        assert!(!result.contains_key("resp_2"));
        assert!(result["resp_3"].unwrap().performance(false).is_none());
        assert!(result["resp_4"].is_none());
    }
}
