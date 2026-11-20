use std::{
    collections::HashMap, ffi::OsString, fs, io, path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering}, time::UNIX_EPOCH,
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

use crate::{
    detail::{read_metered, MeteredFile}, pricing::Pricing,
    quota_history::{store_profile, store_samples, RETENTION_MS}, sources::Sources,
};
use super::super::{account_tag, parse_codex};

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS forecast_files (
        tool TEXT NOT NULL, path TEXT NOT NULL, stamp TEXT NOT NULL,
        modified INTEGER NOT NULL, gap_start INTEGER, gap_end INTEGER,
        PRIMARY KEY(tool,path)
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS forecast_requests (
        tool TEXT NOT NULL, path TEXT NOT NULL, event TEXT NOT NULL, at INTEGER NOT NULL,
        model TEXT NOT NULL, tokens INTEGER NOT NULL, usd REAL, missing INTEGER NOT NULL,
        PRIMARY KEY(tool,path,event)
    ) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS forecast_requests_time ON forecast_requests(tool,at);
    CREATE TABLE IF NOT EXISTS forecast_scans (tool TEXT PRIMARY KEY, at INTEGER NOT NULL);
";

struct SourceFile
{
    path: PathBuf,
    stamp: String,
    modified: i64,
}

fn metadata(path: &Path) -> io::Result<(u128, u64)>
{
    let meta = fs::metadata(path)?;
    let time = meta.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default();
    Ok((time.as_nanos(), meta.len()))
}

fn files(root: &Path, tool: &str, cutoff: i64, depth: usize, output: &mut Vec<SourceFile>) -> io::Result<()>
{
    let entries = match fs::read_dir(root)
    {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    for entry in entries
    {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() && tool == "codex" && depth < 4
        {
            files(&path, tool, cutoff, depth + 1, output)?;
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let matches = if tool == "codex"
        {
            name.starts_with("rollout-") && [".jsonl", ".jsonl.zst", ".jsonl.zstd"].iter().any(|ext| name.ends_with(ext))
        }
        else { name.ends_with(".db") };
        if !kind.is_file() || !matches { continue; }
        let (time, size) = metadata(&path)?;
        let (wal_time, wal_size) = if tool == "antigravity"
        {
            // A committed WAL can change while the database file stays untouched.
            let mut wal = OsString::from(path.as_os_str());
            wal.push("-wal");
            metadata(Path::new(&wal)).unwrap_or_default()
        }
        else { (0, 0) };
        let modified = (time.max(wal_time) / 1_000_000) as i64;
        if modified >= cutoff
        {
            output.push(SourceFile { path, stamp: format!("{time}/{size}/{wal_time}/{wal_size}"), modified });
        }
    }
    Ok(())
}

pub(super) fn price_provider(tool: &str, model: &str) -> &'static str
{
    let model = model.to_ascii_lowercase();
    if tool == "codex" || model.contains("gpt") { "openai" }
    else if model.contains("gemini") || model.contains("argon") { "gemini" }
    else if model.contains("claude") { "anthropic" }
    else { "" }
}

fn store_requests(db: &Connection, tool: &str, path: &Path, file: MeteredFile, prices: &Pricing) -> rusqlite::Result<()>
{
    let source = path.to_string_lossy();
    db.execute("DELETE FROM forecast_requests WHERE tool=?1 AND path=?2", params![tool, source])?;
    let mut insert = db.prepare_cached("INSERT OR IGNORE INTO forecast_requests VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let session = name.split(".jsonl").next().unwrap_or(&name);
    let mut occurrences = HashMap::<i64, usize>::new();
    for request in file.requests
    {
        let event = match request.id
        {
            Some(id) => format!("id/{id}"),
            None =>
            {
                // Stable across archive moves, compression and retention cutoffs.
                let index = occurrences.entry(request.at).or_default();
                *index += 1;
                format!("{session}/{}/{index}", request.at)
            }
        };
        let model = request.model.unwrap_or_default();
        let (usd, missing) = prices.request_cost(price_provider(tool, &model), &model, request.counts);
        insert.execute(params![tool, source, event, request.at, model, request.counts.total(), usd, missing])?;
    }
    Ok(())
}

// Read only the optional public account identifier. Older rollouts often omit it.
pub(super) fn current_codex_account(sources: &Sources) -> Option<String>
{
    #[derive(Deserialize)]
    struct Identity { account_id: Option<String> }
    #[derive(Deserialize)]
    struct Auth { tokens: Option<Identity>, account_id: Option<String> }
    let auth: Auth = serde_json::from_reader(fs::File::open(sources.codex_auth()).ok()?).ok()?;
    account_tag(auth.tokens.and_then(|tokens| tokens.account_id).or(auth.account_id).as_deref())
}

pub(super) fn sync(
    db: &mut Connection, sources: &Sources, tool: &str, prices: &Pricing, now: i64, stopped: &AtomicBool,
) -> Result<(), Box<dyn std::error::Error>>
{
    let cutoff = now - RETENTION_MS;
    let mut candidates = Vec::new();
    for root in sources.roots_for(tool) { files(&root, tool, cutoff, 0, &mut candidates)?; }
    for file in candidates
    {
        if stopped.load(Ordering::Relaxed) { return Err(io::Error::from(io::ErrorKind::Interrupted).into()); }
        let path = file.path.to_string_lossy();
        let previous: Option<(String, i64)> = db.prepare_cached(
            "SELECT stamp,COALESCE(gap_start,modified) FROM forecast_files WHERE tool=?1 AND path=?2")?
            .query_row(params![tool, path], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if previous.as_ref().is_some_and(|(stamp, _)| *stamp == file.stamp) { continue; }
        let mut samples = Vec::new();
        let mut subscription = false;
        let mut other_limit = false;
        let result = read_metered(tool, &file.path, cutoff, |value| {
            if let Some(quota) = parse_codex(value)
            {
                subscription = true;
                if (cutoff..=now).contains(&quota.observed_at)
                {
                    quota.samples(&mut samples);
                }
            }
            else if value.pointer("/payload/rate_limits/limit_id").and_then(|id| id.as_str()).is_some_and(|id| id != "codex")
            {
                other_limit = true;
            }
        });
        // Parsing runs outside the write transaction. A failed reread retains the previous requests.
        let tx = db.transaction()?;
        let mut stamp = file.stamp;
        let gap = match result
        {
            Ok(mut metered) =>
            {
                let mut gap = metered.partial.then_some((cutoff, file.modified.min(now)));
                if tool == "codex" && !subscription
                {
                    // Other providers can also produce Codex transcripts. An unattributed
                    // default-provider session makes its own time range unsuitable for calibration.
                    if !other_limit && !metered.custom_provider && gap.is_none()
                    {
                        let times = metered.requests.iter().map(|r| r.at);
                        gap = times.clone().min().zip(times.max());
                    }
                    metered.requests.clear();
                }
                store_requests(&tx, tool, &file.path, metered, prices)?;
                store_samples(&tx, &samples, now)?;
                gap
            }
            Err(_) =>
            {
                stamp.clear();
                Some((previous.map_or(cutoff, |(_, modified)| modified.min(now)), file.modified.min(now)))
            }
        };
        tx.execute("INSERT OR REPLACE INTO forecast_files VALUES (?1,?2,?3,?4,?5,?6)",
            params![tool, path, stamp, file.modified, gap.map(|g| g.0), gap.map(|g| g.1)])?;
        tx.commit()?;
    }
    let tx = db.transaction()?;
    if tool == "codex"
    {
        if let Some(account) = current_codex_account(sources) { store_profile(&tx, tool, now, None, Some(&account))?; }
    }
    tx.execute("DELETE FROM forecast_requests WHERE tool=?1 AND at<?2", params![tool, cutoff])?;
    tx.execute("DELETE FROM forecast_files WHERE tool=?1 AND modified<?2", params![tool, cutoff])?;
    tx.execute("INSERT OR REPLACE INTO forecast_scans VALUES (?1,?2)", params![tool, now])?;
    tx.commit()?;
    Ok(())
}
