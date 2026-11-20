use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::{add_tool, summary, Conversation, Span, TokenCounts, Turn};

const USER_INPUT: i64 = 14;
const PLANNER_RESPONSE: i64 = 15;
const USER_EXPLICIT: u64 = 4;

#[derive(Default)]
struct Steps
{
    by_index: HashMap<i64, Step>,
    response_index: HashMap<String, i64>,
    generation_index: HashMap<i64, i64>,
    prompts: BTreeMap<i64, (Option<i64>, String)>,
}

#[derive(Default)]
struct Step
{
    time: Option<i64>,
    tools: Vec<String>,
}

pub(super) fn read(paths: &[PathBuf], models: &[String], span: &Span, conversation: &mut Conversation) -> io::Result<bool>
{
    let mut seen = HashSet::new();
    for path in paths
    {
        read_database(path, models, span, conversation, &mut seen).map_err(io::Error::other)?;
    }
    Ok(false)
}

fn read_database(
    path: &Path, models: &[String], span: &Span,
    conversation: &mut Conversation, seen: &mut HashSet<String>,
) -> rusqlite::Result<()>
{
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_millis(500))?;
    // The steps and the generations must come from the same committed WAL view.
    let tx = db.transaction()?;
    let steps = load_steps(&tx)?;
    let fallback = tx.query_row(
        "SELECT data FROM trajectory_metadata_blob LIMIT 1", [], |row| row.get::<_, Vec<u8>>(0),
    ).optional().ok().flatten().and_then(|data| bytes(&data, 2).and_then(timestamp))
        .or_else(|| fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis().try_into().ok());
    let mut select = tx.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx")?;
    let mut generations = select.query([])?;
    let mut last_prompt = None;
    while let Some(row) = generations.next()?
    {
        let index: i64 = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        let Some(chat) = bytes(&blob, 1) else { continue };
        let Some(usage) = bytes(chat, 4) else { continue };
        let counts = parse_usage(usage);
        if counts.total() == 0 { continue; }
        let response = string(usage, 11);
        if response.is_some_and(|id| !seen.insert(id.to_owned())) { continue; }
        let step_id = number(&blob, 2).or_else(|| bytes(&blob, 2).and_then(|mut packed| varint(&mut packed)))
            .and_then(|id| i64::try_from(id).ok())
            .filter(|id| steps.by_index.contains_key(id))
            .or_else(|| response.and_then(|id| steps.response_index.get(id).copied()))
            .or_else(|| steps.generation_index.get(&index).copied());
        let step = step_id.and_then(|id| steps.by_index.get(&id));
        let started_at = bytes(chat, 9).and_then(|time| bytes(time, 4)).and_then(timestamp);
        let at = started_at
            .or_else(|| step.and_then(|step| step.time)).or(fallback);
        if let Some((id, (prompt_at, prompt))) = step_id.and_then(|id| steps.prompts.range(..id).next_back())
        {
            if last_prompt != Some(*id)
            {
                conversation.prompt(prompt.clone(), *prompt_at);
                last_prompt = Some(*id);
            }
        }
        let model = string(chat, 19).filter(|name| !name.eq_ignore_ascii_case("auto"))
            .map(model_alias).map(str::to_owned)
            .or_else(|| (models.len() == 1 && models[0] != "unknown").then(|| models[0].clone()));
        let tools = step.map(|step| step.tools.clone()).unwrap_or_default();
        let timing = started_at.and_then(|_| generation_timing(chat));
        let mut turn = Turn::new(at, model, tools, counts, "reply").with_timing(timing);
        turn.response_id = response.map(str::to_owned);
        conversation.add(span, turn, step_id.is_none() || last_prompt.is_none());
    }
    Ok(())
}

fn generation_timing(chat: &[u8]) -> Option<super::TranscriptTiming>
{
    // The two protobuf durations are first-token wait and the remaining stream.
    fn nanos(mut value: &[u8]) -> Option<u64>
    {
        let (mut seconds, mut nanos) = (0_u64, 0_u64);
        while !value.is_empty()
        {
            match read_field(&mut value)?
            {
                (1, Wire::Number(value)) => seconds = value,
                (2, Wire::Number(value)) => nanos = value,
                (1 | 2, _) => return None,
                _ => {}
            }
        }
        if nanos >= 1_000_000_000 { return None; }
        seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
    }
    let first = nanos(bytes(chat, 11)?)?;
    let elapsed = first.checked_add(nanos(bytes(chat, 12)?)?)? / 1_000_000;
    super::TranscriptTiming::from_elapsed(elapsed, Some(first / 1_000_000))
}

fn load_steps(db: &Connection) -> rusqlite::Result<Steps>
{
    let mut steps = Steps::default();
    let mut columns = db.prepare("PRAGMA table_info(steps)")?;
    let names = columns.query_map([], |row| row.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if names.is_empty() { return Ok(steps); }
    let payload = if names.iter().any(|col| col == "step_payload") { "step_payload" } else { "NULL" };
    let mut select = db.prepare(&format!("SELECT idx, step_type, metadata, {payload} FROM steps WHERE step_type IN (14, 15)"))?;
    let mut rows = select.query([])?;
    while let Some(row) = rows.next()?
    {
        let index: i64 = row.get(0)?;
        let kind: i64 = row.get(1)?;
        let metadata: Option<Vec<u8>> = row.get(2)?;
        let payload: Option<Vec<u8>> = row.get(3)?;
        let metadata = metadata.as_deref().or_else(|| payload.as_deref().and_then(|payload| bytes(payload, 5))).unwrap_or_default();
        let at = bytes(metadata, 1).and_then(timestamp);
        if kind == USER_INPUT
        {
            if number(metadata, 3).is_some_and(|source| source != USER_EXPLICIT) { continue; }
            let user = payload.as_deref().and_then(|blob| bytes(blob, 19));
            let prompt = user.and_then(|blob| string(blob, 1).or_else(|| string(blob, 2))).and_then(prompt_summary);
            if let Some(prompt) = prompt { steps.prompts.insert(index, (at, prompt)); }
        }
        if kind == PLANNER_RESPONSE
        {
            if let Some(response) = bytes(metadata, 9).and_then(|model| string(model, 11))
            {
                steps.response_index.insert(response.to_owned(), index);
            }
            if let Some(generation) = bytes(metadata, 20).and_then(|info| number(info, 3)).and_then(|id| i64::try_from(id).ok())
            {
                steps.generation_index.insert(generation, index);
            }
            let mut tools = Vec::new();
            if let Some(planner) = payload.as_deref().and_then(|blob| bytes(blob, 20))
            {
                let mut reader = planner;
                while let Some((tag, field)) = read_field(&mut reader)
                {
                    if let (7, Wire::Bytes(call)) = (tag, field) { add_tool(&mut tools, string(call, 2)); }
                }
            }
            steps.by_index.insert(index, Step { time: at, tools });
        }
    }
    Ok(steps)
}

fn prompt_summary(prompt: &str) -> Option<String>
{
    let prompt = prompt.split_once("<USER_REQUEST>").and_then(|(_, rest)| rest.split_once("</USER_REQUEST>").map(|(request, _)| request))
        .unwrap_or(prompt);
    summary(prompt)
}

fn parse_usage(usage: &[u8]) -> TokenCounts
{
    // Tokscale's current schema maps field 1 to an enum, field 9 to thinking,
    // and field 10 to response text. The total output field includes both.
    let reasoning = number(usage, 9);
    let output = number(usage, 10).filter(|amount| *amount > 0)
        .or_else(|| number(usage, 3).map(|total| total.saturating_sub(reasoning.unwrap_or(0))));
    TokenCounts {
        input: number(usage, 2), output, cache_write: number(usage, 4), cache_read: number(usage, 5), reasoning,
    }
}

fn model_alias(model: &str) -> &str
{
    match model
    {
        "gemini-3-flash-a" | "gemini-3-flash-b" | "gemini-3-flash-agent" |
        "model_placeholder_m132" | "model_placeholder_m133" => "gemini-3.5-flash-high",
        "gemini-3.5-flash-low" | "model_placeholder_m20" => "gemini-3.5-flash-medium",
        "model_placeholder_m187" => "gemini-3.5-flash-extra-low",
        _ => model,
    }
}

enum Wire<'a>
{
    Number(u64),
    Bytes(&'a [u8]),
    Fixed,
}

fn varint(data: &mut &[u8]) -> Option<u64>
{
    let mut value = 0_u64;
    for shift in (0..64).step_by(7)
    {
        let (&byte, rest) = data.split_first()?;
        *data = rest;
        if shift == 63 && byte > 1 { return None; }
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 128 { return Some(value); }
    }
    None
}

fn read_field<'a>(data: &mut &'a [u8]) -> Option<(u64, Wire<'a>)>
{
    let header = varint(data)?;
    let value = match header & 7
    {
        0 => Wire::Number(varint(data)?),
        2 =>
        {
            let size = usize::try_from(varint(data)?).ok()?;
            let (value, rest) = data.split_at_checked(size)?;
            *data = rest;
            Wire::Bytes(value)
        }
        1 | 5 =>
        {
            *data = data.get(if header & 7 == 1 { 8.. } else { 4.. })?;
            Wire::Fixed
        }
        _ => return None,
    };
    Some((header >> 3, value))
}

fn number(mut data: &[u8], wanted: u64) -> Option<u64>
{
    while let Some((tag, value)) = read_field(&mut data)
    {
        if let (true, Wire::Number(value)) = (tag == wanted, value)
        {
            return (value <= i64::MAX as u64).then_some(value);
        }
    }
    None
}

fn bytes(mut data: &[u8], wanted: u64) -> Option<&[u8]>
{
    while let Some((tag, value)) = read_field(&mut data)
    {
        if let (true, Wire::Bytes(value)) = (tag == wanted, value) { return Some(value); }
    }
    None
}

fn string(data: &[u8], tag: u64) -> Option<&str>
{
    std::str::from_utf8(bytes(data, tag)?).ok().map(str::trim).filter(|value| !value.is_empty())
}

fn timestamp(data: &[u8]) -> Option<i64>
{
    let seconds = number(data, 1)?;
    let nanos = number(data, 2).unwrap_or(0);
    if seconds == 0 || nanos >= 1_000_000_000 { return None; }
    i64::try_from(seconds.checked_mul(1_000)?.checked_add(nanos / 1_000_000)?).ok()
}

#[cfg(test)]
mod tests
{
    use super::*;
    use crate::model::Period;
    use chrono::{DateTime, Utc, Weekday};

    fn var(mut number: u64) -> Vec<u8>
    {
        let mut bytes = Vec::new();
        while number > 127 { bytes.push((number as u8 & 127) | 128); number >>= 7; }
        bytes.push(number as u8);
        bytes
    }

    fn v(field: u64, value: u64) -> Vec<u8>
    {
        [var(field * 8), var(value)].concat()
    }

    fn b(field: u64, value: &[u8]) -> Vec<u8>
    {
        [var(field * 8 + 2), var(value.len() as u64), value.to_vec()].concat()
    }

    #[test]
    fn reads_linked_sqlite_usage_and_prompt_before_checkpoint()
    {
        let path = std::env::temp_dir().join(format!("token-monitor-lite-ag-{}.db", std::process::id()));
        let timestamp = 1_795_108_009_u64;
        let writer = Connection::open(&path).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        writer.execute_batch("CREATE TABLE gen_metadata(idx INTEGER,data BLOB);
            CREATE TABLE steps(idx INTEGER,step_type INTEGER,metadata BLOB,step_payload BLOB);").unwrap();
        let data = [
            v(1, 550), v(2, 20), v(3, 9), v(5, 12), v(9, 7), v(10, 2), b(11, b"fixture-response"),
        ].concat();
        let chat = [b(4, &data), b(9, &b(4, &v(1, timestamp))),
            b(11, &v(2, 250_000_000)), b(12, &v(2, 750_000_000)), b(19, b"gemini-3-flash-a")].concat();
        let planner = b(20, &b(7, &b(2, b"list_directory")));
        let prompt = b(19, &b(1, b"<USER_REQUEST>Inspect the project</USER_REQUEST>"));
        writer.execute("INSERT INTO steps VALUES (0,14,?1,?2)", rusqlite::params![v(3,4),prompt]).unwrap();
        let step = [b(1, &v(1, timestamp)), b(9, &b(11, b"fixture-response"))].concat();
        writer.execute("INSERT INTO steps VALUES (1,15,?1,?2)", rusqlite::params![step,planner]).unwrap();
        writer.execute("INSERT INTO gen_metadata VALUES (0,?1)", [b(1, &chat)]).unwrap();
        let now = DateTime::<Utc>::from_timestamp(timestamp as i64, 0).unwrap();
        let span = Span::new(Period::Today, now, chrono_tz::Asia::Shanghai, Weekday::Mon);
        let mut conversation = Conversation::default();
        assert!(!read(std::slice::from_ref(&path), &[], &span, &mut conversation).unwrap());
        assert_eq!(conversation.exchanges[0].prompt.as_deref(), Some("Inspect the project"));
        let turn = &conversation.exchanges[0].turns[0];
        assert_eq!(turn.tokens, 41);
        assert_eq!(turn.model.as_deref(), Some("gemini-3.5-flash-high"));
        assert_eq!(turn.timing, super::super::TranscriptTiming::from_elapsed(1000, Some(250)));
        assert_eq!(turn.tools, ["list_directory"]);
        drop(writer);
        let _ = fs::remove_file(path);
    }
}
