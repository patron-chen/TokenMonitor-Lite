use super::*;

const T: i64 = 1_800_000_000_000;
const HOUR: i64 = 3_600_000;

fn window(minutes: u64, end: i64) -> QuotaWindow
{
    QuotaWindow { group: String::new(), minutes: Some(minutes), remaining: 80.0, resets_at: Some(end), forecast: None }
}

fn reading(at: i64, remaining: f64, end: i64) -> Reading
{
    Reading { at, value: remaining, reset_at: Some(end) }
}

fn request(at: i64, tokens: u64, usd: f64) -> (i64, Amount)
{
    (at, Amount { tokens, usd: Some(usd), missing: false })
}

fn ledger(events: Vec<(i64, Amount)>, through: i64) -> Ledger
{
    Ledger { since: T - RETENTION_MS, through, groups: HashMap::from([(String::new(), events)]), gaps: vec![] }
}

#[test]
fn pairs_plateau_usage_in_both_windows_and_keeps_hover_totals_independent_of_the_first_sample()
{
    let now = T + HOUR;
    let ledger = ledger(vec![request(T, 125, 0.01), request(T + 300, 80_000, 0.7),
        request(T + 700, 120_000, 1.1), request(T + 1500, 300, 0.09)], now);
    for (minutes, drop, tokens, usd) in [(300, 10.0, 2_000_000, 18.0), (10_080, 5.0, 4_000_000, 36.0)]
    {
        let end = T + 2 * HOUR;
        let points = [reading(T, 85.0, end), reading(T + 400, 85.0, end), reading(T + 900, 85.0 - drop, end)];
        let f = estimate(&window(minutes, end), points[2].at, &points, &ledger, now).unwrap();
        assert_eq!((f.basis, f.total_tokens, f.used_tokens), ("current", Some(tokens), Some(200_425)));
        assert!((f.total_usd.unwrap() - usd).abs() < 1e-9);
        assert!((f.used_usd.unwrap() - 1.90).abs() < 1e-9);
    }
}

#[test]
fn new_cycle_uses_only_previous_valid_intervals_until_it_has_five_points()
{
    let old = T - 5 * HOUR;
    let end = T + 5 * HOUR;
    let rows = [reading(old, 80.0, T), reading(old + HOUR, 70.0, T), reading(T, 100.0, end),
        reading(T + HOUR, 98.0, end), reading(T + 2 * HOUR, 95.0, end)];
    let mut data = ledger(vec![request(old + HOUR, 50_000, 0.3), request(T + HOUR, 1000, 0.01),
        request(T + 2 * HOUR, 4000, 0.04)], T + HOUR);
    let historic = estimate(&window(300, end), rows[3].at, &rows, &data, data.through).unwrap();
    assert_eq!((historic.basis, historic.total_tokens, historic.used_tokens), ("history", Some(500_000), Some(1000)));
    data.through += HOUR;
    let current = estimate(&window(300, end), rows[4].at, &rows, &data, data.through).unwrap();
    assert_eq!((current.basis, current.total_tokens, current.used_tokens), ("current", Some(100_000), Some(5000)));
    assert!(estimate(&window(300, end), rows[4].at, &rows, &data, end).is_none());
    data.since = T;
    let new_account = estimate(&window(300, end), rows[3].at, &rows[..4], &data, data.through).unwrap();
    assert_eq!(new_account.basis, "insufficient");
}

#[test]
fn floating_full_windows_missing_prices_gaps_and_refills_are_distinct()
{
    let end = T + 5 * HOUR;
    let rows = [reading(T - 4000, 100.0, end - 4000), reading(T - 2000, 100.0, end - 2000),
        reading(T + 2000, 93.0, end), reading(T + 3000, 100.0, end), reading(T + 4000, 90.0, end)];
    let mut data = ledger(vec![(T, Amount { tokens: 7000, usd: None, missing: true }), request(T+4000, 1200, 0.2)], T+2000);
    let first = estimate(&window(300, end), rows[2].at, &rows, &data, data.through).unwrap();
    assert_eq!((first.total_tokens, first.total_usd, first.used_usd, first.missing_price), (Some(100_000), None, None, true));
    data.through += 2000;
    let refill = estimate(&window(300, end), rows[4].at, &rows, &data, data.through).unwrap();
    assert_eq!((refill.used_tokens, refill.total_tokens, refill.total_usd), (Some(1200), Some(12_000), Some(2.0)));
    data.gaps.push((T, T+4000));
    let partial = estimate(&window(300, end), rows[4].at, &rows, &data, data.through).unwrap();
    assert!(partial.partial && partial.total_tokens.is_none());
}

#[test]
fn crossing_midnight_and_depleting_a_window_preserves_both_sides_of_midnight()
{
    let midnight = chrono::DateTime::parse_from_rfc3339("2026-11-21T00:00:00+08:00").unwrap().timestamp_millis();
    let end = midnight + HOUR;
    let data = Ledger { since: midnight - RETENTION_MS, through: midnight + 500,
        groups: HashMap::from([(String::new(), vec![request(end - 5 * HOUR - 1, 1000, 2.0),
            request(midnight - 200, 3000, 0.3), request(midnight + 100, 7000, 0.7)])]), gaps: vec![] };
    let points = [reading(midnight - 500, 10.0, end), reading(midnight + 200, 0.0, end)];
    let f = estimate(&window(300, end), points[1].at, &points, &data, data.through).unwrap();
    assert_eq!((f.used_tokens, f.used_usd, f.total_tokens, f.total_usd), (Some(10_000), Some(1.0), Some(100_000), Some(10.0)));
    let fresh = window(300, end + 5 * HOUR);
    let f = estimate(&fresh, end, &points, &data, end).unwrap();
    assert_eq!((f.basis, f.used_tokens, f.total_tokens), ("history", None, Some(100_000)));
}

#[test]
fn groups_unknown_models_and_account_boundaries_are_isolated()
{
    let groups = ["Gemini", "Gemini 4 Argon", "Claude/GPT"].map(String::from);
    assert_eq!(model_group("antigravity", "gemini-4-argon", &groups), Some("Gemini 4 Argon"));
    assert_eq!(model_group("antigravity", "gemini-3.5-flash", &groups), Some("Gemini"));
    assert_eq!(model_group("antigravity", "claude-opus", &groups), Some("Claude/GPT"));
    assert_eq!(model_group("antigravity", "auto", &groups), None);
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE quota_profiles(tool TEXT,at INTEGER,plan TEXT,account TEXT,PRIMARY KEY(tool,at))").unwrap();
    for (at, plan, account) in [(1, Some("pro"), Some("a")), (2, Some("pro"), None),
        (3, None, Some("b")), (4, Some("pro"), Some("b")), (5, Some("plus"), Some("b"))]
    {
        quota_history::store_profile(&db, "codex", at, plan, account).unwrap();
        assert_eq!(scope_start(&db, "codex", 0).unwrap(), match at { 1 | 2 => 0, 3 | 4 => 3, _ => 5 });
    }
}

#[test]
fn archived_codex_requests_survive_restart_and_duplicate_active_rollouts()
{
    use std::fs;
    use chrono::Utc;
    use serde_json::json;
    use crate::{quota::QuotaCache, settings::QuotaSettings};

    let now = Utc::now().timestamp_millis();
    let root = std::env::temp_dir().join(format!("lite-quota-forecast-{}-{now}", std::process::id()));
    let sources = Sources::from_home(Some(root.clone()));
    let active = sources.codex_sessions();
    let archive = sources.codex_root().join("archived_sessions");
    fs::create_dir_all(&active).unwrap();
    fs::create_dir_all(&archive).unwrap();
    let name = "rollout-forecast-fixture.jsonl";
    let archived = archive.join(format!("{name}.zst"));
    let live = active.join(name);
    let at = now - HOUR;
    let usage = json!({"input_tokens":180_000,"cached_input_tokens":30_000,"output_tokens":20_000,"reasoning_output_tokens":5_000});
    let limits = |five, week| json!({"limit_id":"codex","plan_type":"plus",
        "primary":{"window_minutes":300,"used_percent":five,"resets_at":at+4*HOUR},
        "secondary":{"window_minutes":10080,"used_percent":week,"resets_at":at+72*HOUR}});
    let lines = [
        json!({"type":"turn_context","timestamp":at,"payload":{"model":"gpt-5.4"}}),
        json!({"type":"event_msg","timestamp":at,"payload":{"type":"token_count","rate_limits":limits(5,10)}}),
        json!({"type":"token_usage_record","timestamp":at+500,"payload":{"response_id":"forecast-response","usage":usage}}),
        json!({"type":"event_msg","timestamp":at+500,"payload":{"type":"token_count","rate_limits":limits(15,15),
            "info":{"total_token_usage":usage,"last_token_usage":usage}}}),
    ].map(|row| row.to_string()).join("\n");
    fs::write(&archived, zstd::stream::encode_all(lines.as_bytes(), 1).unwrap()).unwrap();
    let tools = ToolSelection { codex: true, antigravity: false, dsh: false };
    let settings = QuotaSettings { tools, ..QuotaSettings::default() };
    for pass in 0..3
    {
        if pass == 2 { fs::write(&live, &lines).unwrap(); }
        let cache = QuotaCache::new(&root);
        cache.sample_local(&sources, tools);
        let snapshot = cache.read(&sources, settings, true);
        let values = &snapshot.tools[0].windows;
        assert_eq!(values.len(), 2);
        for (row, factor) in values.iter().zip([10,20])
        {
            let f = row.forecast.as_ref().unwrap();
            assert_eq!((f.basis, f.used_tokens, f.total_tokens), ("current", Some(200_000), Some(200_000 * factor)));
            assert!(f.used_usd.unwrap() > 0.0 && (f.total_usd.unwrap() - f.used_usd.unwrap() * factor as f64).abs() < 1e-9);
        }
    }
    fs::remove_file(live).unwrap();
    fs::remove_file(archived).unwrap();
    fs::remove_file(root.join("quota-history.sqlite")).unwrap();
    fs::remove_file(root.join("logs/codex-banked-reset.log")).unwrap();
    fs::remove_dir(root.join("logs")).unwrap();
    fs::remove_dir(active).unwrap();
    fs::remove_dir(archive).unwrap();
    fs::remove_dir(sources.codex_root()).unwrap();
    fs::remove_dir(root).unwrap();
}
