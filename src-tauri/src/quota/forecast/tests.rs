use std::sync::Arc;

use crate::app_log::{AppLog, LOG_FILE};
use super::*;

const T: i64 = 1_800_000_000_000;
const HOUR: i64 = 3_600_000;

fn window(minutes: u64, end: i64) -> QuotaWindow
{
    QuotaWindow { group: String::new(), minutes: Some(minutes), remaining: 80.0, resets_at: Some(end), forecast_reset_at: None, forecast: None }
}

fn reading(at: i64, remaining: f64, end: i64) -> Reading
{
    Reading { at, value: remaining, reset_at: Some(end) }
}

fn request(at: i64, tokens: u64, usd: f64) -> Request
{
    Request { at, amount: Amount { tokens, usd: Some(usd), missing: false }, model: None, speed_factor: 1.0 }
}

fn unpriced(at: i64, tokens: u64) -> Request
{
    Request { amount: Amount { tokens, usd: None, missing: true }, ..request(at, tokens, 0.0) }
}

fn ledger(events: Vec<Request>, through: i64) -> Ledger
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
    let mut data = ledger(vec![unpriced(T, 7000), request(T+4000, 1200, 0.2)], T+2000);
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
fn manual_reset_relearns_from_new_pairs_and_keeps_all_cycle_usage()
{
    let end = T + 5 * HOUR;
    let mut selected = window(300, end);
    selected.forecast_reset_at = Some(T + 2000);
    let points = [reading(T - 4 * HOUR, 80.0, T), reading(T - 3 * HOUR, 70.0, T),
        reading(T, 90.0, end), reading(T + 1000, 80.0, end), reading(T + 3000, 60.0, end),
        reading(T + 4000, 58.0, end), reading(T + 5000, 55.0, end)];
    let mut data = ledger(vec![request(T - 3 * HOUR, 50_000, 0.5),
        unpriced(T + 700, 90_000),
        request(T + 2500, 30_000, 0.3), request(T + 4000, 2000, 0.04), request(T + 5000, 3000, 0.06)], T + 4000);
    let early = estimate(&selected, data.through, &points, &data, data.through).unwrap();
    assert_eq!((early.basis, early.total_tokens, early.used_tokens), ("insufficient", None, Some(122_000)));
    data.through = T + 5000;
    let ready = estimate(&selected, data.through, &points, &data, data.through).unwrap();
    assert_eq!((ready.basis, ready.used_tokens, ready.total_tokens), ("current", Some(125_000), Some(100_000)));
    assert!(ready.missing_price && (ready.used_usd.unwrap() - 0.4).abs() < 1e-9);
    assert!((ready.total_usd.unwrap() - 2.0).abs() < 1e-9);
}

#[test]
fn reset_persists_for_only_the_selected_tool_group_and_duration()
{
    use crate::{quota::{Providers, QuotaCache, QuotaRefresh, Quotas}, settings::QuotaSettings};
    use std::fs;

    let now = chrono::Utc::now().timestamp_millis();
    let root = std::env::temp_dir().join(format!("lite-forecast-reset-{}-{now}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let tools = ToolSelection { codex: true, antigravity: true, dsh: false };
    let db = QuotaHistory::new(&root).connection().unwrap();
    db.execute_batch(index::SCHEMA).unwrap();
    for (tool, model) in [("codex", "gpt"), ("antigravity", "gemini-pro"), ("antigravity", "claude-opus")]
    {
        db.execute("INSERT OR REPLACE INTO forecast_scans VALUES (?1,?2)", params![tool,now]).unwrap();
        db.execute("INSERT INTO forecast_requests VALUES (?1,'test',?2,?3,?2,1000,1,0)", params![tool,model,now-1000]).unwrap();
    }
    let refresh = |cache: &QuotaCache| {
        let provider = |tool, groups: &[&str]| {
            let mut windows = Vec::new();
            for group in groups
            {
                for minutes in [300, 10_080]
                {
                    let mut w = window(minutes, now + HOUR);
                    w.group = (*group).to_owned();
                    windows.push(w);
                }
            }
            ToolQuota { tool, plan: None, account: None, source: None, observed_at: now, windows, balances: vec![], banked_reset: None }
        };
        let codex = provider("codex", &[""]);
        let ag = provider("antigravity", &["Gemini", "Claude/GPT"]);
        let mut samples = Vec::new();
        for mut baseline in [codex.clone(), ag.clone()]
        {
            baseline.observed_at = now - 2000;
            for w in &mut baseline.windows { w.remaining += 10.0; }
            baseline.samples(&mut samples);
        }
        cache.refresh(QuotaSettings { tools, ..QuotaSettings::default() }, QuotaRefresh::Panel,
            || (Some(codex), Providers { antigravity: Some(ag), ..Providers::default() }, samples))
    };
    let check = |snapshot: &Quotas, after_reset| {
        assert_eq!(snapshot.tools.iter().map(|p| p.windows.len()).sum::<usize>(), 6);
        for p in &snapshot.tools
        {
            for w in &p.windows
            {
                let selected = after_reset && p.tool == "antigravity" && w.group == "Gemini" && w.minutes == Some(10_080);
                let f = w.forecast.as_ref().unwrap();
                assert_eq!(w.forecast_reset_at.is_some(), selected);
                assert_eq!((f.used_tokens, f.used_usd), (Some(1000), Some(1.0)));
                assert_eq!((f.total_tokens, f.total_usd), ((!selected).then_some(10_000), (!selected).then_some(10.0)));
            }
        }
    };
    let key = QuotaKey::Window { tool: "antigravity".into(), group: "Gemini".into(), minutes: Some(10_080) };
    let log = Arc::new(AppLog::new(&root));
    let cache = QuotaCache::new(&root, log.clone());
    check(&refresh(&cache), false);
    let history = quota_history::read(&db, &key, now).unwrap();
    check(&cache.reset_forecast(&key, tools).unwrap(), true);
    check(&cache.cached(tools).unwrap(), true);
    check(&refresh(&QuotaCache::new(&root, log)), true);
    let after = quota_history::read(&db, &key, now).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().map(|p| (p.at, p.value, p.reset_at)).eq(after.iter().map(|p| (p.at, p.value, p.reset_at))));
    drop(db);
    fs::remove_file(root.join("quota-history.sqlite")).unwrap();
    fs::remove_file(root.join("logs").join(LOG_FILE)).unwrap();
    fs::remove_dir(root.join("logs")).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
fn archived_codex_requests_survive_restart_and_duplicate_active_rollouts()
{
    use std::fs;
    use chrono::Utc;
    use serde_json::json;
    use crate::{quota::{QuotaCache, QuotaRefresh}, settings::QuotaSettings};

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
        json!({"type":"turn_context","timestamp":at,"payload":{"model":"gpt-5.4","service_tier":"priority"}}),
        json!({"type":"event_msg","timestamp":at,"payload":{"type":"token_count","rate_limits":limits(5,10)}}),
        json!({"type":"token_usage_record","timestamp":at+500,"payload":{"response_id":"forecast-response","service_tier":"ultrafast","usage":usage}}),
        json!({"type":"event_msg","timestamp":at+500,"payload":{"type":"token_count","rate_limits":limits(5,10),
            "info":{"total_token_usage":usage,"last_token_usage":usage}}}),
        json!({"type":"turn_context","timestamp":at+600,"payload":{"model":"gpt-5.4"}}),
        json!({"type":"token_usage_record","timestamp":at+700,"payload":{"response_id":"next-response","usage":usage}}),
        json!({"type":"event_msg","timestamp":at+800,"payload":{"type":"token_count","rate_limits":limits(15,15)}}),
    ].map(|row| row.to_string()).join("\n");
    fs::write(&archived, zstd::stream::encode_all(lines.as_bytes(), 1).unwrap()).unwrap();
    let tools = ToolSelection { codex: true, antigravity: false, dsh: false };
    let settings = QuotaSettings { tools, ..QuotaSettings::default() };
    for pass in 0..3
    {
        if pass == 2 { fs::write(&live, &lines).unwrap(); }
        let cache = QuotaCache::new(&root, Arc::new(AppLog::new(&root)));
        cache.sample_local(&sources, tools, "test", true);
        let snapshot = cache.read(&sources, settings, QuotaRefresh::Panel);
        let values = &snapshot.tools[0].windows;
        assert_eq!(values.len(), 2);
        let db = QuotaHistory::new(&root).connection().unwrap();
        let tiers: (usize, usize) = db.query_row("SELECT COUNT(*),COUNT(CASE WHEN tier='ultrafast' THEN 1 END)
            FROM forecast_request_tiers", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let copies = if pass == 2 { 2 } else { 1 };
        assert_eq!(tiers, (copies, copies)); // The next turn did not specify a tier.
        for (row, factor) in values.iter().zip([10,20])
        {
            let f = row.forecast.as_ref().unwrap();
            assert_eq!((f.basis, f.used_tokens, f.total_tokens), ("current", Some(400_000), Some(400_000 * factor)));
            assert!(f.used_usd.unwrap() > 0.0 && (f.total_usd.unwrap() - f.used_usd.unwrap() * factor as f64).abs() < 1e-9);
        }
    }
    fs::remove_file(live).unwrap();
    fs::remove_file(archived).unwrap();
    fs::remove_file(root.join("quota-history.sqlite")).unwrap();
    fs::remove_file(root.join("logs").join(LOG_FILE)).unwrap();
    fs::remove_dir(root.join("logs")).unwrap();
    fs::remove_dir(active).unwrap();
    fs::remove_dir(archive).unwrap();
    fs::remove_dir(sources.codex_root()).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
fn recent_model_mix_projects_only_remaining_quota_in_either_window()
{
    for minutes in [300, 10_080]
    {
        let duration = minutes as i64 * 60_000;
        let old = T - duration;
        let end = T + duration;
        let mut rows = vec![reading(old, 100.0, T)];
        let mut history = Vec::new();
        // Model A gets 200 raw tokens per quota point, model B gets 40.
        for (i, model) in [0, 1, 0, 1, 0, 1, 0].into_iter().enumerate()
        {
            let time = old + (i + 1) as i64;
            let tokens = if model == 0 { 1000 } else { 200 };
            history.push(Request { model: Some(model), ..request(time, tokens, tokens as f64 / 1000.0) });
            rows.push(reading(time, 95.0 - 5.0 * i as f64, T));
        }
        rows.push(reading(T, 100.0, end));
        for (model, remaining, total) in [(0, 99.0, 20_000), (1, 95.0, 4000)]
        {
            let now = T + 20;
            let mut events = history.clone();
            for time in T + 1..=now
            {
                events.push(Request { model: Some(model), ..request(time, 10, 0.01) });
            }
            let mut points = rows.clone();
            points.push(reading(now, remaining, end));
            let mut selected = window(minutes, end);
            selected.remaining = remaining;
            let mut data = ledger(events, now);
            let forecast = estimate(&selected, now, &points, &data, now).unwrap();
            assert_eq!((forecast.used_tokens, forecast.total_tokens, forecast.model_mix_requests),
                (Some(200), Some(total), Some(20)));
            assert!((forecast.total_usd.unwrap() - total as f64 / 1000.0).abs() < 1e-8);

            // A new model and a manual reset each independently invalidate the learned mix.
            data.groups.get_mut("").unwrap().last_mut().unwrap().model = Some(2);
            let unknown = estimate(&selected, now, &points, &data, now).unwrap();
            assert!(unknown.total_tokens.is_some() && unknown.model_mix_requests.is_none());
            data.groups.get_mut("").unwrap().last_mut().unwrap().model = Some(model);
            selected.forecast_reset_at = Some(T);
            assert!(estimate(&selected, now, &points, &data, now).unwrap().model_mix_requests.is_none());
        }

        let mut events = history;
        events.push(Request { model: Some(0), ..request(T + 1, 1000, 1.0) });
        for time in T + 2..=T + 21
        {
            events.push(Request { model: Some(1), ..request(time, 10, 0.01) });
        }
        rows.extend([reading(T + 1, 95.0, end), reading(T + 21, 90.0, end)]);
        let mut selected = window(minutes, end);
        selected.remaining = 90.0;
        let data = ledger(events, T + 21);
        let switched = estimate(&selected, data.through, &rows, &data, data.through).unwrap();
        // The first 1000 tokens retain their measured spend; the last 20 calls price the remaining 90 points.
        assert_eq!((switched.used_tokens, switched.total_tokens, switched.model_mix_requests),
            (Some(1200), Some(4800), Some(20)));
        assert!((switched.total_usd.unwrap() - 4.8).abs() < 1e-8);
    }
}

#[test]
fn only_documented_tiers_share_a_rate_and_only_codex_auto_review_is_free()
{
    let mut models = Models::default();
    let (model, standard) = models.identify("codex", "gpt-6.2-astra", Some("standard"));
    assert_eq!(standard, 1.0);
    assert_eq!(models.identify("codex", "GPT-6.2-Astra", Some("priority")), (model, 2.0));
    assert_eq!(models.identify("codex", "gpt-6.2-astra", Some("ultrafast")), (model, 6.0));
    assert_eq!(models.identify("codex", "gpt-6.1-sol", Some("fast")).1, 2.5);
    assert_eq!(models.identify("codex", "gpt-6.1-sol", Some("ultrafast")).1, 8.0);
    let unspecified = models.identify("codex", "gpt-6.2-astra", None);
    assert_ne!(unspecified.0, model);
    assert_ne!(models.identify("codex", "gpt-6.2-astra", Some("flex")).0, unspecified.0);
    assert_eq!(models.identify("antigravity", "gpt-6.2-astra", Some("fast")).1, 1.0);
    assert_eq!(models.identify("codex", "codex-auto-review", None), (None, 0.0));
    assert_eq!(models.identify("codex", "code-review", None).1, 1.0);
    let end = T + 5 * HOUR;
    let rows = [reading(T, 90.0, end), reading(T + 2, 80.0, end), reading(T + 3, 75.0, end)];
    let mut data = ledger(vec![Request { speed_factor: 0.0, ..unpriced(T + 1, 10_000) },
        request(T + 2, 1000, 1.0), Request { speed_factor: 0.0, ..unpriced(T + 3, 20_000) }], T + 3);
    let f = estimate(&window(300, end), data.through, &rows, &data, data.through).unwrap();
    assert_eq!((f.total_tokens, f.total_usd, f.used_tokens, f.missing_price), (Some(10_000), Some(10.0), Some(31_000), true));
    data.groups.get_mut("").unwrap().retain(|r| !r.billable());
    assert!(estimate(&window(300, end), data.through, &rows, &data, data.through).unwrap().total_tokens.is_none());
}

#[test]
fn speed_scales_once_and_validation_rejects_noisy_model_rates()
{
    let history = Calibration::default();
    let work = |model, tokens, speed_factor| Workload::from_requests([Request {
        model: Some(model), speed_factor, ..request(T, tokens, 0.1)
    }].iter());
    let mut current = Calibration::default();
    for _ in 0..2
    {
        current.observe(&work(0, 100, 1.0), 5.0);
        current.observe(&work(1, 1000, 1.0), 5.0);
    }
    assert_eq!(current.predict_points(&history, &work(0, 100, 2.0)), Some(10.0));
    let mut validation = Validation::default();
    for _ in 0..3
    {
        let later = work(0, 100, 1.0);
        validation.compare(&later, 1.0, &current, &history);
        current.observe(&later, 1.0);
    }
    assert!(!validation.accepted());
}
