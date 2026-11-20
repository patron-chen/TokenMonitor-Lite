use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::{
    detail::time,
    model::{reasoning_effort, Session, SessionActivity, SessionContext},
    sources::{codex_index_id, Sources},
};

const RECENT_MS: i64 = 10 * 60_000;
const LIVE_TAIL_BYTES: u64 = 512 * 1024;
const SETTING_TAIL_BYTES: u64 = 96 * 1024;

pub fn enrich(sources: &Sources, rows: &mut [Session])
{
    let catalog = read_catalog(sources.codex_root(), rows);
    let now = Utc::now().timestamp_millis();
    for row in rows
    {
        if row.tool != "codex"
        {
            row.native_id = Some(row.id.clone());
            continue;
        }
        row.native_id = codex_index_id(&row.id).map(str::to_owned);
        if let Some(saved) = row.native_id.as_deref().and_then(|id| catalog.get(id))
        {
            row.reasoning_effort.clone_from(&saved.effort);
            row.latest_model.clone_from(&saved.model);
            row.archived = saved.archived;
            row.metrics.background_review |= saved.background_review;
            if row.title.is_empty()
            {
                row.title = saved.title.clone().unwrap_or_default();
            }
        }
        let recent = row.last_used_at.is_some_and(|at| now.saturating_sub(at) <= RECENT_MS);
        if row.archived || (!recent && row.reasoning_effort.is_some()) { continue; }
        let Some(file) = sources.codex_file(&row.id) else { continue };
        if file.components().any(|part| part.as_os_str() == "archived_sessions")
        {
            row.archived = true;
            if row.reasoning_effort.is_some() { continue; }
        }
        if file.extension().is_none_or(|extension| extension != "jsonl") { continue; }
        let Some(tail) = read_tail(&file, if recent { LIVE_TAIL_BYTES } else { SETTING_TAIL_BYTES }) else { continue };
        let status = read_status(&tail);
        // A current turn's explicit setting (including a clear) overrides its catalog.
        if let Some(setting) = status.effort { row.reasoning_effort = setting; }
        if status.model.is_some() { row.latest_model = status.model; }
        if recent && !row.archived
        {
            row.activity = status.activity;
            row.context = status.context;
        }
    }
}

#[derive(Default)]
struct CatalogRow
{
    title: Option<String>,
    effort: Option<String>,
    model: Option<String>,
    archived: bool,
    background_review: bool,
}

fn catalog_paths(root: &Path) -> Vec<PathBuf>
{
    let mut found = Vec::new();
    for dir in [root.to_owned(), root.join("sqlite")]
    {
        let mut candidates: Vec<_> = fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let version = name.to_str()?.strip_prefix("state_")?.strip_suffix(".sqlite")?.parse::<u32>().ok()?;
                entry.path().is_file().then(|| (version, entry.path()))
            }).collect();
        candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        found.extend(candidates.into_iter().map(|(_, path)| path));
    }
    found
}

fn read_catalog(root: &Path, rows: &[Session]) -> HashMap<String, CatalogRow>
{
    let mut found = HashMap::new();
    let ids: Vec<_> = rows.iter().filter(|row| row.tool == "codex").filter_map(|row| codex_index_id(&row.id)).collect();
    if ids.is_empty() { return found; }
    for path in catalog_paths(root)
    {
        let Ok(connection) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else { continue };
        let Ok(mut schema) = connection.prepare("PRAGMA table_info(threads)") else { continue };
        let Ok(columns) = schema.query_map([], |row| row.get::<_, String>(1)) else { continue };
        let columns: HashSet<_> = columns.filter_map(Result::ok).collect();
        if !columns.contains("id") { continue; }
        // Older state databases have only a subset of these display fields.
        let fields = ["name", "title", "reasoning_effort", "model", "archived", "thread_source"].map(|field| {
            if columns.contains(field) { field } else { "NULL" }
        });
        let Ok(mut query) = connection.prepare(&format!("SELECT {} FROM threads WHERE id = ?1", fields.join(", "))) else { continue };
        for &id in &ids
        {
            if found.contains_key(id) { continue; }
            if let Ok(row) = query.query_row([id], |row| {
                let title = row.get::<_, Option<String>>(0)?.filter(|name| !name.trim().is_empty())
                    .or(row.get(1)?).map(|title| title.trim().chars().take(160).collect());
                Ok(CatalogRow {
                    title,
                    effort: row.get::<_, Option<String>>(2)?.and_then(|value| reasoning_effort(&value)),
                    model: row.get::<_, Option<String>>(3)?.filter(|value| !value.is_empty()),
                    archived: row.get::<_, Option<bool>>(4)?.unwrap_or(false),
                    background_review: row.get::<_, Option<String>>(5)?.is_some_and(|source| source == "guardian_review"),
                })
            })
            {
                found.insert(id.to_owned(), row);
            }
        }
        if ids.iter().all(|id| found.contains_key(*id)) { break; }
    }
    found
}

fn read_tail(path: &Path, limit: u64) -> Option<Vec<u8>>
{
    let mut file = File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(limit);
    file.seek(SeekFrom::Start(start.saturating_sub(1))).ok()?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    if start > 0
    {
        let first_full = bytes.iter().position(|byte| *byte == b'\n')? + 1;
        bytes.drain(..first_full);
    }
    Some(bytes)
}

#[derive(Default)]
struct Status
{
    effort: Option<Option<String>>,
    model: Option<String>,
    activity: Option<SessionActivity>,
    context: Option<SessionContext>,
}

fn read_status(tail: &[u8]) -> Status
{
    let mut status = Status::default();
    let mut waiting = HashSet::new();
    for line in tail.split(|byte| *byte == b'\n')
    {
        if line.is_empty() || line.len() > 64 * 1024 { continue; }
        let Ok(value) = serde_json::from_slice::<Value>(line) else { continue };
        let data = &value["payload"];
        let at = time(&value["timestamp"]);
        if value["type"] == "turn_context"
        {
            let effort = data.get("effort").unwrap_or(&data["reasoning_effort"]);
            status.effort = Some(effort.as_str().and_then(reasoning_effort));
            status.model = data["model"].as_str().filter(|model| !model.is_empty()).map(str::to_owned);
        }
        let Some(at) = at else { continue };
        match (value["type"].as_str(), data["type"].as_str())
        {
            (Some("event_msg"), Some("task_started" | "turn_started")) =>
            {
                waiting.clear();
                status.activity = Some(SessionActivity { state: "running", at });
            }
            (Some("event_msg"), Some("task_complete" | "turn_aborted" | "turn_completed")) =>
            {
                waiting.clear();
                status.activity = Some(SessionActivity { state: "ended", at });
            }
            (Some("event_msg"), Some("token_count")) =>
            {
                let info = &data["info"];
                let usage = info.get("last_token_usage").unwrap_or(&info["lastTokenUsage"]);
                let tokens = usage["total_tokens"].as_u64().or_else(|| usage["totalTokens"].as_u64());
                let window = ["model_context_window", "modelContextWindow", "context_window", "contextWindow"]
                    .into_iter().find_map(|key| info[key].as_u64());
                if let Some((tokens, window)) = tokens.zip(window).filter(|(tokens, window)| *tokens > 0 && *window > 0)
                {
                    status.context = Some(SessionContext { tokens, window, at });
                }
                if let Some(activity) = status.activity.as_mut().filter(|activity| activity.state == "running")
                {
                    activity.at = activity.at.max(at);
                }
            }
            (Some("response_item"), Some("function_call")) if data["name"] == "request_user_input" =>
            {
                if let Some(id) = data["call_id"].as_str()
                {
                    waiting.insert(id.to_owned());
                    status.activity = Some(SessionActivity { state: "waiting", at });
                }
            }
            (Some("response_item"), Some("function_call_output")) =>
            {
                if data["call_id"].as_str().is_some_and(|id| waiting.remove(id)) && waiting.is_empty()
                {
                    status.activity = Some(SessionActivity { state: "running", at });
                }
            }
            _ => {}
        }
    }
    status
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn codex_tail_shows_the_latest_effort_and_uses_per_call_context()
    {
        let tail = br#"{"type":"turn_context","payload":{"effort":"low","model":"gpt-old"}}
{"timestamp":"2026-11-20T02:00:00Z","type":"event_msg","payload":{"type":"task_started"}}
{"type":"turn_context","payload":{"effort":"max","model":"gpt-new"}}
{"timestamp":"2026-11-20T02:00:10Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":52000},"total_token_usage":{"total_tokens":822000},"model_context_window":260000}}}
{"timestamp":"2026-11-20T02:00:12Z","type":"event_msg","payload":{"type":"task_complete"}}"#;
        let status = read_status(tail);
        assert_eq!(status.effort, Some(Some("max".into())));
        assert_eq!(status.model.as_deref(), Some("gpt-new"));
        assert_eq!(status.activity.unwrap().state, "ended");
        let context = status.context.unwrap();
        assert_eq!((context.tokens, context.window), (52_000, 260_000));
        assert_eq!(read_status(br#"{"type":"turn_context","payload":{"effort":null}}"#).effort, Some(None));
    }
}
