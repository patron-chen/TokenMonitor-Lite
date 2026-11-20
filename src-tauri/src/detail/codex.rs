use std::{collections::HashSet, io, path::Path};

use serde_json::Value;

use crate::{model::reasoning_effort, sources::codex_index_id};

use super::{add_tool, content_prompt, image_label, number, records_matching, summary, text, time,
    Conversation, Span, TokenCounts, TranscriptTiming, Turn};

pub(super) fn read(path: &Path, span: &Span, conversation: &mut Conversation) -> io::Result<bool>
{
    read_observing(path, span, conversation, |_| {})
}

pub(super) fn read_observing(
    path: &Path, span: &Span, conversation: &mut Conversation, mut observe: impl FnMut(&Value),
) -> io::Result<bool>
{
    let metering = conversation.metering.is_some();
    let mut parser = Parser::new(span, conversation);
    let result = records_matching(path, false, |line| !metering || metering_line(line), |value| {
        parser.record(&value);
        if !parser.fork_prefix && value["payload"]["rate_limits"].is_object()
        {
            observe(&value);
        }
        true
    });
    parser.flush_pending();
    result
}

fn metering_line(line: &[u8]) -> bool
{
    // Large tool results and assistant output do not affect request counts. Unrecognized
    // JSON layouts still go through the normal parser.
    let head = &line[..line.len().min(256)];
    let marker = br#""type":"response_item","payload":{"type":""#;
    let Some(offset) = head.windows(marker.len()).position(|part| part == marker) else { return true };
    let kind = &head[offset + marker.len()..];
    ![b"function_call".as_slice(), b"custom_tool_call", b"reasoning", br#"message","role":"assistant"#]
        .iter().any(|prefix| kind.starts_with(prefix))
}

struct Pending
{
    turn: Turn,
    usage: [u64; 4],
}

#[derive(Default)]
struct RequestClock
{
    boundary: Option<i64>,
    start: Option<i64>,
    model_end: Option<i64>,
    tool_result: Option<i64>,
    last_count: Option<i64>,
    empty: Option<bool>,
    first_output: Option<i64>,
    first_was_tool: Option<bool>,
    reasoning_exposed: bool,
}

impl RequestClock
{
    fn start_turn(&mut self, at: Option<i64>)
    {
        *self = Self { boundary: at, ..Self::default() };
    }

    fn response(&mut self, payload: &Value, at: Option<i64>)
    {
        let Some(kind) = payload["type"].as_str() else { return };
        if kind == "reasoning" || kind.ends_with("_call") || (kind == "message" && payload["role"] == "assistant")
        {
            // Old rollouts can flush the previous request's items just after its count.
            if self.start.is_none() && at.zip(self.last_count).is_some_and(|(at, count)| (0..=100).contains(&(at - count)))
            {
                return;
            }
            let empty = kind == "message" && payload["content"].as_array().is_some_and(|parts|
                parts.iter().all(|part| part["type"] == "output_text" && part["text"] == ""));
            self.empty = Some(self.empty.unwrap_or(true) && empty);
            self.first_was_tool.get_or_insert(kind.ends_with("_call"));
            if kind == "reasoning"
            {
                // A summary can be streamed after unseen reasoning.
                self.reasoning_exposed |= payload["content"].as_array()
                    .is_some_and(|parts| parts.iter().any(|part| text(&part["text"]).is_some()));
            }
            if let Some(at) = at
            {
                if self.start.is_none() { self.start = self.boundary; }
                self.model_end = Some(at);
                self.tool_result = None;
            }
        }
        else if kind.ends_with("_output") || kind == "message"
        {
            self.boundary = self.boundary.max(at);
            if self.model_end.is_some() && kind.ends_with("_output") { self.tool_result = at; }
        }
    }

    fn output_item(&mut self, payload: &Value)
    {
        // New rollouts report when a reasoning or text item began, while the
        // JSONL record itself is written after that item has finished.
        if matches!(payload["item"]["type"].as_str(), Some("Reasoning" | "AgentMessage"))
        {
            if let Some(at) = time(&payload["started_at_ms"])
            {
                self.first_output = Some(self.first_output.map_or(at, |previous| previous.min(at)));
            }
        }
    }

    fn finish(&mut self, at: Option<i64>, reasoning: u64, from_record: bool) -> Option<TranscriptTiming>
    {
        let start = self.start.or(self.boundary);
        // A token_count can be delayed until a local tool finishes. The last
        // model item is the end of generation in that flush.
        let same_flush = at.zip(self.tool_result).is_some_and(|(at, result)| (0..=100).contains(&(at - result)));
        let end = if !from_record && same_flush { self.model_end } else { at };
        let valid = (self.empty != Some(true) || reasoning > 0) &&
            (!from_record || at.zip(self.model_end).is_none_or(|(at, end)| at >= end));
        let first_output = if self.first_was_tool == Some(true) { None } else { self.first_output };
        let last_count = if from_record { self.last_count } else { self.last_count.max(at) };
        *self = Self { boundary: self.boundary.max(at), last_count, ..Self::default() };
        if !valid { return None; }
        TranscriptTiming::from_events(start?, end?, first_output)
    }
}

struct Parser<'a>
{
    span: &'a Span,
    conversation: &'a mut Conversation,
    model: Option<String>,
    effort: Option<String>,
    session: Option<String>,
    fork_prefix: bool,
    tools: Vec<String>,
    latest_total: Option<[u64; 4]>,
    clock: RequestClock,
    pending: Option<Pending>,
    response_ids: HashSet<String>,
    previous_prompt: Option<(bool, String)>,
    other_pool: bool,
}

impl<'a> Parser<'a>
{
    fn new(span: &'a Span, conversation: &'a mut Conversation) -> Self
    {
        Self {
            span, conversation, model: None, effort: None, session: None, fork_prefix: false,
            tools: Vec::new(), latest_total: None, clock: RequestClock::default(), pending: None,
            response_ids: HashSet::new(), previous_prompt: None, other_pool: false,
        }
    }

    fn record(&mut self, value: &Value)
    {
        let payload = &value["payload"];
        let at = time(&value["timestamp"]);
        let previous = self.previous_prompt.take();
        match value["type"].as_str()
        {
            Some("session_meta") =>
            {
                self.session = text(&payload["id"]).map(str::to_owned);
                if let Some(meter) = &mut self.conversation.metering
                {
                    meter.custom_provider = text(&payload["model_provider"]).is_some_and(|id| !id.eq_ignore_ascii_case("openai"));
                }
                if self.conversation.native_id.is_none()
                {
                    self.conversation.native_id = self.session.as_deref().and_then(codex_index_id).map(str::to_owned);
                }
                self.fork_prefix = text(&payload["forked_from_id"]).is_some()
                    || payload.pointer("/source/subagent/thread_spawn").is_some();
            }
            Some("turn_context") =>
            {
                self.flush_pending();
                self.fork_prefix = false;
                self.model = model(payload).or_else(|| self.model.clone());
                self.effort = text(payload.get("effort").unwrap_or(&payload["reasoning_effort"])).and_then(reasoning_effort);
                self.clock.start_turn(at);
            }
            _ if self.fork_prefix =>
            {
                if payload["type"] == "token_count"
                {
                    self.latest_total = raw_usage(&payload["info"]["total_token_usage"]);
                }
            }
            Some("token_usage_record") =>
            {
                if text(&payload["thread_id"]).zip(self.session.as_deref()).is_some_and(|(thread, own)| thread != own)
                {
                    return;
                }
                self.flush_pending();
                let usage = &payload["usage"];
                if let Some(raw) = raw_usage(usage)
                {
                    let counts = split(raw, usage);
                    let model = model(payload).or_else(|| self.model.clone());
                    let reasoning_exposed = self.clock.reasoning_exposed;
                    let timing = self.clock.finish(at, counts.reasoning.unwrap_or(0), true);
                    let mut turn = Turn::new(at, model, std::mem::take(&mut self.tools), counts, "reply").with_timing(timing);
                    turn.reasoning_effort.clone_from(&self.effort);
                    turn.response_id = text(&payload["response_id"]).map(str::to_owned);
                    turn.reasoning_exposed = reasoning_exposed;
                    self.pending = Some(Pending { turn, usage: raw });
                }
            }
            Some("event_msg") =>
            {
                match payload["type"].as_str()
                {
                    Some("user_message") => self.user_prompt(event_prompt(payload), true, at, previous),
                    Some("task_started") => self.clock.start_turn(at),
                    Some("task_complete" | "turn_aborted") =>
                    {
                        let last_count = self.clock.last_count;
                        self.clock.start_turn(None);
                        self.clock.last_count = last_count;
                    }
                    Some("token_count") => self.count(payload, at),
                    Some("item_completed") => self.clock.output_item(payload),
                    Some("mcp_tool_call_end") => add_tool(&mut self.tools, text(&payload["name"]).or_else(|| text(&payload["invocation"]["tool"]))),
                    _ => {}
                }
            }
            Some("response_item") =>
            {
                self.clock.response(payload, at);
                match payload["type"].as_str()
                {
                    Some("function_call" | "custom_tool_call" | "tool_search_call") =>
                        add_tool(&mut self.tools, text(&payload["name"])),
                    Some("message") if payload["role"] == "user" =>
                        self.user_prompt(response_prompt(payload), false, at, previous),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn user_prompt(&mut self, prompt: Option<String>, native: bool, at: Option<i64>, previous: Option<(bool, String)>)
    {
        let Some(prompt) = prompt else { return };
        if native { self.clock.start_turn(at); }
        // Codex writes adjacent UI and API copies of the same user event.
        if !previous.is_some_and(|(kind, text)| kind != native && text == prompt)
        {
            self.flush_pending();
            self.conversation.prompt(prompt.clone(), at);
        }
        self.previous_prompt = Some((native, prompt));
    }

    fn count(&mut self, payload: &Value, at: Option<i64>)
    {
        if payload["rate_limits"].is_object()
        {
            self.other_pool = payload["rate_limits"]["limit_id"].as_str().is_some_and(|id| id != "codex");
        }
        let info = &payload["info"];
        let last = raw_usage(&info["last_token_usage"]);
        let total = raw_usage(&info["total_token_usage"]);
        let mut raw = last;
        if let Some(total) = total
        {
            if let Some(prior) = self.latest_total
            {
                if total == prior
                {
                    self.pending = None;
                    return;
                }
                if total.iter().zip(prior).all(|(new, old)| *new >= old)
                {
                    raw = Some(std::array::from_fn(|i| total[i] - prior[i]));
                }
                else if last.is_some_and(|delta| total[0] + total[1] + 2 * (delta[0] + delta[1]) >= prior[0] + prior[1])
                {
                    self.pending = None;
                    return;
                }
            }
            else if raw.is_none()
            {
                raw = Some(total);
            }
            self.latest_total = Some(total);
        }
        let Some(raw) = raw else { return };
        let fields = if last.is_some() { &info["last_token_usage"] } else { &info["total_token_usage"] };
        let counts = split(raw, fields);
        let candidate = self.pending.take();
        let mut model = model(payload).or_else(|| self.model.clone());
        let mut effort = self.effort.clone();
        let mut tools = std::mem::take(&mut self.tools);
        let mut response_id = None;
        let reasoning_exposed;
        let timing = if let Some(pending) = candidate
        {
            if let Some(ref id) = pending.turn.response_id
            {
                if !self.response_ids.insert(id.clone()) { return; }
            }
            model = pending.turn.model.or(model);
            effort = pending.turn.reasoning_effort.or(effort);
            for name in pending.turn.tools { add_tool(&mut tools, Some(&name)); }
            self.clock.last_count = self.clock.last_count.max(at);
            reasoning_exposed = pending.turn.reasoning_exposed;
            if raw == pending.usage && last == Some(raw) && pending.turn.at.zip(at).is_none_or(|(before, after)| before <= after)
            {
                response_id = pending.turn.response_id;
                pending.turn.timing
            }
            else { None }
        }
        else
        {
            reasoning_exposed = self.clock.reasoning_exposed;
            let timing = self.clock.finish(at, counts.reasoning.unwrap_or(0), false);
            timing.filter(|_| last.is_none_or(|usage| usage == raw))
        };
        let mut turn = Turn::new(at, model, tools, counts, "reply").with_timing(timing);
        turn.reasoning_effort = effort;
        turn.response_id = response_id;
        turn.reasoning_exposed = reasoning_exposed;
        if !self.other_pool || self.conversation.metering.is_none()
        {
            self.conversation.add(self.span, turn, false);
        }
    }

    fn flush_pending(&mut self)
    {
        if let Some(pending) = self.pending.take()
        {
            if (!self.other_pool || self.conversation.metering.is_none())
                && pending.turn.response_id.as_ref().is_none_or(|id| self.response_ids.insert(id.clone()))
            {
                self.conversation.add(self.span, pending.turn, false);
            }
        }
    }
}

fn model(payload: &Value) -> Option<String>
{
    text(&payload["model"]).or_else(|| text(&payload["response_model"]))
        .or_else(|| text(&payload["model_slug"])).map(str::to_owned)
}

fn raw_usage(value: &Value) -> Option<[u64; 4]>
{
    let values = [
        number(&value["input_tokens"]), number(&value["output_tokens"]),
        cache_read(value), number(&value["reasoning_output_tokens"]),
    ];
    values.iter().any(Option::is_some).then(|| values.map(|value| value.unwrap_or(0)))
}

fn cache_read(value: &Value) -> Option<u64>
{
    number(&value["cached_input_tokens"]).max(number(&value["cache_read_input_tokens"]))
}

fn split([input, output, cache, reason]: [u64; 4], fields: &Value) -> TokenCounts
{
    let cache = cache.min(input);
    let reason = reason.min(output);
    TokenCounts {
        input: (input > 0 || number(&fields["input_tokens"]).is_some()).then_some(input - cache),
        output: (output > 0 || number(&fields["output_tokens"]).is_some()).then_some(output - reason),
        cache_read: (cache > 0 || cache_read(fields).is_some()).then_some(cache),
        cache_write: None,
        reasoning: (reason > 0 || number(&fields["reasoning_output_tokens"]).is_some()).then_some(reason),
    }
}

fn event_prompt(payload: &Value) -> Option<String>
{
    let message = text(&payload["message"]).or_else(|| text(&payload["text"])).unwrap_or("");
    let images = ["images", "local_images"].iter().filter_map(|key| payload[*key].as_array()).map(Vec::len).sum();
    user_text(&format!("{} {message}", image_label(images)))
}

fn response_prompt(payload: &Value) -> Option<String>
{
    let kinds = payload.pointer("/internal_chat_message_metadata_passthrough/content_item_kinds").and_then(Value::as_array);
    let parts = payload["content"].as_array()?.iter().enumerate().filter(|(index, _)| {
        kinds.is_none_or(|kinds| kinds.get(*index).and_then(Value::as_str).is_some_and(|kind| kind.starts_with("user.")))
    }).map(|(_, part)| part);
    content_prompt(parts, "input_text", "input_image").and_then(|prompt| user_text(&prompt))
}

fn user_text(message: &str) -> Option<String>
{
    let message = message.split_once("## My request for Codex:").map_or(message, |(_, request)| request).trim();
    if message.starts_with("<environment_context>") || message.starts_with("<user_instructions>") ||
       message.starts_with("# AGENTS.md instructions") || message.starts_with("<system-reminder>")
    {
        return None;
    }
    summary(message)
}

#[cfg(test)]
mod tests
{
    use super::*;
    use chrono::{Utc, Weekday};
    use serde_json::json;
    use crate::model::Period;

    #[test]
    fn merges_codex_response_records_and_splits_reasoning_once()
    {
        let span = Span::new(Period::AllTime, Utc::now(), chrono_tz::UTC, Weekday::Mon);
        let mut conversation = Conversation::default();
        let mut parser = Parser::new(&span, &mut conversation);
        let usage = json!({"input_tokens":100,"output_tokens":10,"cached_input_tokens":20,"reasoning_output_tokens":4});
        let rows = [
            json!({"type":"turn_context","payload":{"model":"gpt-5.4"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Fix this"}}),
            json!({"type":"token_usage_record","payload":{"response_id":"one","usage":usage}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":usage,"total_token_usage":usage}}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":usage,"total_token_usage":usage}}}),
        ];
        for row in rows { parser.record(&row); }
        parser.flush_pending();
        assert_eq!(conversation.exchanges.len(), 1);
        assert_eq!(conversation.exchanges[0].turns[0].counts.output, Some(6));
        assert_eq!(conversation.exchanges[0].turns[0].counts.cache_read, Some(20));
        assert_eq!(conversation.exchanges[0].tokens, 110);
    }

    #[test]
    fn log_clock_skips_tool_waits_and_uses_first_output_only_for_supported_items()
    {
        let span = Span::new(Period::AllTime, Utc::now(), chrono_tz::UTC, Weekday::Mon);
        let mut conversation = Conversation::default();
        let mut parser = Parser::new(&span, &mut conversation);
        let usage = json!({"input_tokens":100,"output_tokens":10,"cached_input_tokens":20,"reasoning_output_tokens":4});
        let total = json!({"input_tokens":200,"output_tokens":20,"cached_input_tokens":40,"reasoning_output_tokens":8});
        let rows = [
            (1, "turn_context", json!({"model":"gpt-5.4","effort":"high"})),
            (2, "event_msg", json!({"type":"user_message","message":"Check timed turns"})),
            (4, "response_item", json!({"type":"function_call","name":"test"})),
            (4, "event_msg", json!({"type":"item_completed","item":{"type":"Reasoning"},
                "started_at_ms":time(&json!("2026-11-20T02:00:03Z")).unwrap()})),
            (24, "response_item", json!({"type":"function_call_output"})),
            (24, "event_msg", json!({"type":"token_count","info":{"last_token_usage":usage,"total_token_usage":usage}})),
            (26, "event_msg", json!({"type":"item_completed","item":{"type":"Reasoning"},
                "started_at_ms":time(&json!("2026-11-20T02:00:25Z")).unwrap()})),
            (26, "response_item", json!({"type":"reasoning"})),
            (26, "token_usage_record", json!({"response_id":"b","usage":usage})),
            (29, "event_msg", json!({"type":"token_count","info":{"last_token_usage":usage,"total_token_usage":total}})),
        ];
        for (seconds, kind, payload) in rows
        {
            parser.record(&json!({"timestamp":format!("2026-11-20T02:00:{seconds:02}Z"),"type":kind,"payload":payload}));
        }
        let turns = &conversation.exchanges[0].turns;
        assert_eq!(turns.len(), 2);
        assert!(turns.iter().all(|turn| turn.reasoning_effort.as_deref() == Some("high")));
        assert_eq!(turns[0].timing, TranscriptTiming::from_elapsed(2000, None));
        assert_eq!(turns[1].timing, TranscriptTiming::from_elapsed(2000, Some(1000)));
        assert_eq!(turns[1].response_id.as_deref(), Some("b"));
    }
}
