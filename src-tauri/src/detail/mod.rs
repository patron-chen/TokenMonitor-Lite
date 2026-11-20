mod antigravity;
mod codex;
mod dsh;
mod timing;

use std::{fs::File, io::{self, BufRead, BufReader, Read}, path::Path};

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{model::{Session, Span, TokenCounts, PAGE_SIZE}, sources::Sources};
use timing::{Performance, TranscriptTiming};

const MAX_LINE: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Sort
{
    #[default]
    Newest,
    Tokens,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail
{
    pub session: Session,
    pub exchanges: Vec<Exchange>,
    pub total: usize,
    pub status: &'static str,
    pub observed_tokens: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exchange
{
    id: usize,
    prompt: Option<String>,
    prompt_at: Option<i64>,
    last_used_at: Option<i64>,
    tokens: u64,
    turns: Vec<Turn>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Turn
{
    at: Option<i64>,
    model: Option<String>,
    tools: Vec<String>,
    counts: TokenCounts,
    tokens: u64,
    kind: &'static str,
    reasoning_effort: Option<String>,
    performance: Option<Performance>,
    #[serde(skip)]
    timing: Option<TranscriptTiming>,
    #[serde(skip)]
    response_id: Option<String>,
    #[serde(skip)]
    reasoning_exposed: bool,
}

impl Turn
{
    fn new(at: Option<i64>, model: Option<String>, tools: Vec<String>, counts: TokenCounts, kind: &'static str) -> Self
    {
        Self {
            at, model, tools, counts, tokens: counts.total(), kind, reasoning_effort: None,
            performance: None, timing: None, response_id: None, reasoning_exposed: false,
        }
    }

    fn with_timing(mut self, timing: Option<TranscriptTiming>) -> Self
    {
        self.timing = timing;
        self
    }
}

#[derive(Default)]
struct Conversation
{
    exchanges: Vec<Exchange>,
    active: Option<usize>,
    next_prompt: Option<(String, Option<i64>)>,
    native_id: Option<String>,
    metering: Option<MeteredFile>,
}

pub(crate) struct MeteredRequest
{
    pub at: i64,
    pub model: Option<String>,
    pub id: Option<String>,
    pub counts: TokenCounts,
}

#[derive(Default)]
pub(crate) struct MeteredFile
{
    pub requests: Vec<MeteredRequest>,
    pub partial: bool,
    pub custom_provider: bool,
    since: i64,
}

/// Use the same de-duplicated turns as session detail, retaining only metering fields.
pub(crate) fn read_metered(
    tool: &str, path: &Path, since: i64, observe_quota: impl FnMut(&Value),
) -> io::Result<MeteredFile>
{
    let span = Span { zone: chrono_tz::UTC, start: None, end: None };
    let mut conversation = Conversation {
        metering: Some(MeteredFile { since, ..MeteredFile::default() }), ..Conversation::default()
    };
    let partial = if tool == "codex"
    {
        codex::read_observing(path, &span, &mut conversation, observe_quota)?
    }
    else
    {
        antigravity::read(&[path.to_owned()], &[], &span, &mut conversation)?
    };
    let mut file = conversation.metering.unwrap();
    file.partial |= partial;
    Ok(file)
}

impl Conversation
{
    fn prompt(&mut self, text: String, at: Option<i64>)
    {
        if self.metering.is_some() { return; }
        self.active = None;
        self.next_prompt = Some((text, at));
    }

    fn add(&mut self, span: &Span, turn: Turn, standalone: bool)
    {
        if !span.contains(turn.at) || turn.tokens == 0 { return; }
        if let Some(meter) = &mut self.metering
        {
            let Some(at) = turn.at else { meter.partial = true; return };
            if at >= meter.since
            {
                meter.requests.push(MeteredRequest { at, model: turn.model, id: turn.response_id, counts: turn.counts });
            }
            return;
        }
        if standalone
        {
            let mut group = self.empty_exchange(None);
            group.add(turn);
            self.exchanges.push(group);
            return;
        }
        if self.active.is_none()
        {
            let prompt = self.next_prompt.take();
            self.exchanges.push(self.empty_exchange(prompt));
            self.active = Some(self.exchanges.len() - 1);
        }
        self.exchanges[self.active.expect("active exchange")].add(turn);
    }

    fn empty_exchange(&self, prompt: Option<(String, Option<i64>)>) -> Exchange
    {
        let (prompt, prompt_at) = prompt.map(|(text, at)| (Some(text), at)).unwrap_or_default();
        Exchange { id: self.exchanges.len(), prompt, prompt_at, last_used_at: None, tokens: 0, turns: Vec::new() }
    }
}

impl Exchange
{
    fn add(&mut self, turn: Turn)
    {
        self.last_used_at = self.last_used_at.max(turn.at);
        self.tokens += turn.tokens;
        self.turns.push(turn);
    }
}

pub fn read(sources: &Sources, mut session: Session, span: &Span, sort: Sort, page: u32) -> Detail
{
    let mut conversation = Conversation::default();
    let result = match session.tool.as_str()
    {
        "codex" => sources.codex_file(&session.id)
            .map(|file| codex::read(&file, span, &mut conversation)),
        "dsh" => dsh::find(sources, &session.id)
            .map(|file| dsh::read(&file, span, &mut conversation)),
        "antigravity" =>
        {
            let files = sources.antigravity_files(&session.id, &session.sources);
            (!files.is_empty()).then(|| antigravity::read(&files, &session.models, span, &mut conversation))
        }
        _ => None,
    };
    if conversation.native_id.is_some() { session.native_id = conversation.native_id; }
    let observed_tokens = conversation.exchanges.iter().map(|group| group.tokens).sum();
    let status = match result
    {
        None => "missing",
        Some(Err(_)) if observed_tokens == 0 => "unreadable",
        Some(Ok(true) | Err(_)) => "partial",
        Some(Ok(false)) if observed_tokens != session.usage.tokens => "partial",
        Some(Ok(false)) => "ready",
    };
    let mut exchanges = conversation.exchanges;
    let total = exchanges.len();
    exchanges.sort_by(|a, b| {
        let by_time = || b.last_used_at.cmp(&a.last_used_at).then_with(|| b.id.cmp(&a.id));
        match sort
        {
            Sort::Newest => by_time(),
            Sort::Tokens => b.tokens.cmp(&a.tokens).then_with(by_time),
        }
    });
    let offset = page as usize * PAGE_SIZE as usize;
    let mut exchanges: Vec<_> = exchanges.into_iter().skip(offset).take(PAGE_SIZE as usize).collect();
    timing::populate(sources, &session, &mut exchanges);
    Detail { session, exchanges, status, total, observed_tokens }
}

fn records(path: &Path, tolerate_end: bool, record: impl FnMut(Value) -> bool) -> io::Result<bool>
{
    records_matching(path, tolerate_end, |_| true, record)
}

fn records_matching(
    path: &Path, tolerate_end: bool, keep: impl Fn(&[u8]) -> bool, mut record: impl FnMut(Value) -> bool,
) -> io::Result<bool>
{
    let mut source = BufReader::new(File::open(path)?);
    let compressed = source.fill_buf()?.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]);
    if compressed
    {
        let mut decoder = zstd::stream::read::Decoder::with_buffer(source)?;
        decoder.window_log_max(26)?;
        read_lines(BufReader::new(decoder), tolerate_end, keep, &mut record)
    }
    else
    {
        read_lines(source, false, keep, &mut record)
    }
}

fn read_lines(
    mut input: impl BufRead, tolerate_end: bool, keep: impl Fn(&[u8]) -> bool, mut record: impl FnMut(Value) -> bool,
) -> io::Result<bool>
{
    let mut line = Vec::new();
    loop
    {
        line.clear();
        match (&mut input).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut line)
        {
            Ok(0) => return Ok(false),
            Ok(_) if !keep(&line) =>
            {
                if line.last() != Some(&b'\n') { input.skip_until(b'\n')?; }
                if line.capacity() > 1024 * 1024 { line = Vec::new(); }
                continue;
            }
            Ok(_) if line.len() > MAX_LINE => return Err(io::Error::new(io::ErrorKind::InvalidData, "Transcript record too large")),
            Ok(_) => {}
            Err(error) if tolerate_end && (error.kind() == io::ErrorKind::UnexpectedEof || error.to_string().contains("incomplete frame")) => return Ok(true),
            Err(error) => return Err(error),
        }
        if let Ok(record_value) = serde_json::from_slice(&line)
        {
            if !record(record_value) { return Ok(false); }
        }
        // One enormous JSONL row should not determine the rest of the scan's working set.
        if line.capacity() > 1024 * 1024 { line = Vec::new(); }
    }
}

fn text(value: &Value) -> Option<&str>
{
    value.as_str().map(str::trim).filter(|text| !text.is_empty())
}

pub(crate) fn time(value: &Value) -> Option<i64>
{
    value.as_i64().or_else(|| DateTime::parse_from_rfc3339(value.as_str()?).ok().map(|time| time.timestamp_millis()))
        .filter(|value| *value > 0)
}

fn summary(text: &str) -> Option<String>
{
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = clean.chars();
    let mut preview: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() { preview.push('…'); }
    (!preview.is_empty()).then_some(preview)
}

fn content_prompt<'a>(parts: impl Iterator<Item = &'a Value>, textual: &str, image: &str) -> Option<String>
{
    let mut words = Vec::new();
    let mut images = 0;
    for part in parts
    {
        match part["type"].as_str()
        {
            Some(kind) if kind == textual =>
            {
                if let Some(word) = text(&part["text"]) { words.push(word.to_owned()); }
            }
            Some(kind) if kind == image => images += 1,
            _ => {}
        }
    }
    if images > 0 { words.insert(0, image_label(images)); }
    summary(&words.join(" "))
}

fn image_label(count: usize) -> String
{
    match count
    {
        0 => String::new(),
        1 => "[image]".to_owned(),
        _ => format!("[{count} images]"),
    }
}

fn number(value: &Value) -> Option<u64>
{
    value.as_u64().filter(|value| *value <= i64::MAX as u64)
}

fn add_tool(tools: &mut Vec<String>, name: Option<&str>)
{
    if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty())
    {
        if !tools.iter().any(|value| value == name)
        {
            tools.push(name.chars().take(100).collect());
        }
    }
}
