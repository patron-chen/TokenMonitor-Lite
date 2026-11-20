use std::{collections::{HashMap, HashSet}, io, path::{Path, PathBuf}};

use serde_json::Value;

use super::{add_tool, content_prompt, number, records, text, time, Conversation, Span, TokenCounts, Turn};
use crate::sources::Sources;

pub(super) fn find(sources: &Sources, id: &str) -> Option<PathBuf>
{
    sources.dsh_files(id).into_iter().find(|path| {
        let mut match_directory = path.parent().and_then(Path::file_name).is_some_and(|name| name == id);
        let _ = records(path, true, |head| {
            if head["type"] == "session"
            {
                if let Some(session) = text(&head["id"]) { match_directory = session == id; }
            }
            false
        });
        match_directory
    })
}

pub(super) fn read(path: &Path, span: &Span, conversation: &mut Conversation) -> io::Result<bool>
{
    let mut lineage = Lineage::default();
    let mut record_index = 0;
    records(path, true, |row| {
        record_index += 1;
        if lineage.header == 0 && row["type"] == "session"
        {
            lineage.header = record_index;
            lineage.cut = row["seedLength"].as_i64();
            lineage.tagged = lineage.cut.is_none() && row["isSeeded"] == true;
            return lineage.tagged;
        }
        if lineage.tagged && row["type"] == "session/end-seed" && row["data"]["inherited"] == true
        {
            lineage.cut = row["seq"].as_i64().or(lineage.cut);
        }
        true
    })?;
    let mut parser = Parser {
        span, conversation, lineage, index: 0, seen: HashSet::new(),
        model: None, provider: None, starts: HashMap::new(), settled: None,
    };
    records(path, true, |row| { parser.record(&row); true })
}

#[derive(Default)]
struct Lineage
{
    header: usize,
    cut: Option<i64>,
    tagged: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Reply, Attempt, Compaction }

struct Parser<'a>
{
    span: &'a Span,
    conversation: &'a mut Conversation,
    lineage: Lineage,
    index: usize,
    seen: HashSet<(String, i64, Option<String>, Option<String>, TokenCounts)>,
    model: Option<String>,
    provider: Option<String>,
    starts: HashMap<(i64, i64), i64>,
    settled: Option<((i64, i64), Kind)>,
}

impl Parser<'_>
{
    fn record(&mut self, row: &Value)
    {
        self.index += 1;
        let seq = row["seq"].as_i64();
        if self.lineage.header > 0 && self.index > self.lineage.header &&
            ((self.lineage.tagged && self.lineage.cut.is_none()) ||
                seq.zip(self.lineage.cut).is_some_and(|(seq, cut)| seq < cut))
        {
            return;
        }
        let data = &row["data"];
        let at = time(&row["time"]);
        let slot = data["turn"].as_i64().zip(data["step"].as_i64());
        let kind = match row["type"].as_str()
        {
            Some("assistant/message") => Kind::Reply,
            Some("assistant/attempt") => Kind::Attempt,
            Some("compaction/summary") => Kind::Compaction,
            Some("request/header") =>
            {
                let configuration = &data["header"]["config"];
                self.model = text(&configuration["model"]).map(str::to_owned).or_else(|| self.model.take());
                self.provider = text(&configuration["provider"]).map(str::to_owned).or_else(|| self.provider.take());
                return;
            }
            Some("user/message") =>
            {
                if data["source"]["kind"] == "user"
                {
                    if let Some(parts) = data["content"].as_array()
                    {
                        if let Some(prompt) = content_prompt(parts.iter(), "text", "image")
                        {
                            self.conversation.prompt(prompt, at);
                        }
                    }
                }
                return;
            }
            Some("step/start" | "llm/retry-started") =>
            {
                if let Some(slot) = slot
                {
                    if let Some(at) = at { self.starts.insert(slot, at); }
                    else { self.starts.remove(&slot); }
                }
                if row["type"] == "llm/retry-started" && self.settled.is_some_and(|(prior, _)| Some(prior) == slot)
                {
                    self.settled = None;
                }
                return;
            }
            _ => return,
        };
        let Some(at) = at else { return };
        let Some(usage) = usage(data, kind) else { return };
        let reasoning = number(&usage["reasoningTokens"]);
        let counts = TokenCounts {
            input: number(&usage["inputTokens"]),
            output: number(&usage["outputTokens"]).map(|out| out.saturating_sub(reasoning.unwrap_or(0))),
            cache_read: number(&usage["cacheReadTokens"]),
            cache_write: number(&usage["cacheWriteTokens"]),
            reasoning,
        };
        if counts.total() == 0 { return; }
        let source = &data["message"]["source"];
        let model = source.pointer("/replayState/response/responseModel").and_then(text)
            .filter(|value| !value.eq_ignore_ascii_case("keepalive"))
            .or_else(|| text(&source["model"])).map(str::to_owned).or_else(|| self.model.clone());
        let provider = text(&source["provider"]).map(str::to_owned).or_else(|| self.provider.clone());
        let identity = match kind
        {
            Kind::Compaction => text(&data["compactionId"]).map(|id| format!("compaction:{id}")),
            Kind::Attempt => text(&data["attemptId"]).or_else(|| text(&data["retryId"])).map(|id| format!("attempt:{id}")),
            Kind::Reply => text(&data["message"]["id"]).map(str::to_owned),
        }.unwrap_or_else(|| format!("{}:{}", kind as u8, seq.unwrap_or_default()));
        if !self.seen.insert((identity, at, provider, model.clone(), counts))
        {
            return;
        }
        if let Some(slot) = slot.filter(|_| kind != Kind::Compaction)
        {
            // A second settlement for the same step is a replay unless a retry
            // boundary or a failed attempt shows that a new call took place.
            if self.settled == Some((slot, kind)) { return; }
            self.settled = Some((slot, kind));
        }
        let mut tools = Vec::new();
        if kind != Kind::Compaction
        {
            for part in data["message"]["content"].as_array().into_iter().flatten()
            {
                if part["type"] == "tool-call" { add_tool(&mut tools, text(&part["name"])); }
            }
        }
        let timestamp = slot.filter(|_| kind != Kind::Compaction).and_then(|slot| self.starts.get(&slot).copied())
            .filter(|start| *start < at).unwrap_or(at);
        let label = match kind
        {
            Kind::Reply => "reply",
            Kind::Attempt => "attempt",
            Kind::Compaction => "compaction",
        };
        // A compact-stream attempt can settle after its outer record's time.
        // Only a reply with direct usage reliably gives both ends here.
        let mut timing = None;
        if kind == Kind::Reply && data["usage"].is_object() && timestamp < at
        {
            let first = data["stream"].as_array().into_iter().flatten()
                .flat_map(|item| [&item["time0"], &item["time"]]).filter_map(time)
                .filter(|event| *event > timestamp).min();
            timing = super::TranscriptTiming::from_events(timestamp, at, first);
        }
        let mut turn = Turn::new(Some(timestamp), model, tools, counts, label).with_timing(timing);
        if kind == Kind::Reply
        {
            turn.response_id = [source.pointer("/replayState/response/responseId"), data["message"].get("id")]
                .into_iter().flatten().find_map(text).map(str::to_owned);
            turn.reasoning_exposed = data["message"]["content"].as_array().into_iter().flatten()
                .any(|part| part["type"] == "reasoning" && text(&part["text"]).is_some());
        }
        self.conversation.add(self.span, turn, kind == Kind::Compaction);
    }
}

fn usage(data: &Value, kind: Kind) -> Option<&Value>
{
    if kind != Kind::Attempt && data["usage"].is_object()
    {
        return Some(&data["usage"]);
    }
    data["stream"].as_array()?.iter().rev()
        .filter_map(|event| event.pointer("/chunk/usage")).find(|usage| usage.is_object())
}

#[cfg(test)]
mod tests
{
    use super::*;
    use chrono::{Utc, Weekday};
    use serde_json::json;
    use crate::model::Period;

    #[test]
    fn dsh_uses_last_tagged_cut_and_keeps_real_paid_requests()
    {
        let span = Span::new(Period::AllTime, Utc::now(), chrono_tz::UTC, Weekday::Mon);
        let mut conversation = Conversation::default();
        let mut parser = Parser {
            span: &span, conversation: &mut conversation,
            lineage: Lineage { header: 1, cut: Some(6), tagged: true }, index: 0,
            seen: HashSet::new(), model: None, provider: None, starts: HashMap::new(), settled: None,
        };
        let head = json!({"type":"session","id":"child","isSeeded":true});
        let call = |seq: i64| json!({
            "type":"assistant/message","time":1787000000000_i64,"seq":seq,
            "data":{"turn":1,"step":1,"message":{"id":"id","source":{"model":"deepseek-reasoner"}},
                "usage":{"inputTokens":100,"outputTokens":15,"reasoningTokens":10},
                "stream":[{"time0":1786999999400_i64,"time":1786999999450_i64}]}
        });
        let start = json!({"type":"step/start","time":1786999999000_i64,"seq":6,"data":{"turn":1,"step":1}});
        for row in [head, call(4), start, call(7), call(7)] { parser.record(&row); }
        assert_eq!(conversation.exchanges[0].tokens, 115);
        assert_eq!(conversation.exchanges[0].turns[0].counts.output, Some(5));
        assert_eq!(conversation.exchanges[0].turns[0].counts.reasoning, Some(10));
        assert_eq!(conversation.exchanges[0].turns[0].timing, super::super::TranscriptTiming::from_elapsed(1000, Some(400)));
    }

    #[test]
    fn keeps_complete_zstd_frame_before_a_torn_tail()
    {
        use std::io::{BufReader, Cursor};
        let mut encoded = zstd::encode_all(&b"{\"seq\":1}\n"[..], 1).unwrap();
        let torn = zstd::encode_all(&b"{\"seq\":2,\"extra\":\"long incomplete string\"}\n"[..], 1).unwrap();
        encoded.extend_from_slice(&torn[..10]);
        let decoded = zstd::stream::read::Decoder::new(Cursor::new(encoded)).unwrap();
        let mut seen = Vec::new();
        let partial = super::super::read_lines(BufReader::new(decoded), true, |_| true, |row| { seen.push(row["seq"].as_i64()); true }).unwrap();
        assert!(partial);
        assert_eq!(seen, [Some(1)]);
    }
}
