use serde_json::json;
use super::*;
use crate::quota::powershell_json;

fn endpoint(rank: u8, port: u16) -> Endpoint
{
    Endpoint { rank, port, secure: true, csrf: String::new() }
}

#[test]
fn loopback_rpcs_share_the_http_worker_and_bypass_the_shortcut_proxy()
{
    use std::{io::{BufRead, BufReader, Read, Write}, net::{TcpListener, TcpStream}, thread};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut calls = Vec::new();
        for client in listener.incoming()
        {
            let mut client = client.unwrap();
            let mut reader = BufReader::new(&client);
            let mut first = String::new();
            if reader.read_line(&mut first).unwrap() == 0 { break; }
            let mut size = 0;
            let mut authenticated = false;
            for line in reader.by_ref().lines().map(Result::unwrap).take_while(|line| !line.is_empty())
            {
                let (name, value) = line.split_once(':').unwrap();
                if name.eq_ignore_ascii_case("Content-Length") { size = value.trim().parse().unwrap(); }
                if name.eq_ignore_ascii_case("X-Codeium-Csrf-Token") { authenticated = value.trim() == "local-fixture"; }
            }
            if !first.starts_with("POST ")
            {
                // The RPC endpoint only accepts POST. Any other host-side probe must carry no service token.
                assert!(!authenticated);
                client.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                continue;
            }
            let mut body = vec![0; size];
            reader.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            calls.push((first, authenticated, body));
            client.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}").unwrap();
        }
        calls
    });
    let ep = Endpoint { rank: 0, port: address.port(), secure: false, csrf: "local-fixture".into() };
    let script = concat!("$env:TOKEN_MONITOR_LITE_PROXY_SERVER = 'http://127.0.0.1:1'\n",
        include_str!("../../../../scripts/windows-http.ps1"), "\nRead-LiteHttpRequests");
    let result = with_http_worker(script, Instant::now() + Duration::from_secs(8), |worker| {
        let summary = request(worker, &ep, Rpc::Summary, Duration::from_secs(2))?;
        let status = request(worker, &ep, Rpc::UserStatus, Duration::from_secs(2))?;
        Some((summary, status))
    });
    drop(TcpStream::connect(address).unwrap());
    let calls = server.join().unwrap();
    assert_eq!(result, Some((json!({"ok":true}), json!({"ok":true}))));
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0].0.starts_with("POST /exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary "));
    assert!(calls[1].0.starts_with("POST /exa.language_server_pb.LanguageServerService/GetUserStatus "));
    assert!(calls.iter().all(|(_, authenticated, _)| *authenticated));
    assert_eq!(calls[0].2["forceRefresh"], true);
    assert_eq!(calls[1].2["metadata"]["ideName"], "antigravity");
}

#[test]
fn unusable_first_summary_does_not_hide_a_grouped_zero_or_require_a_plan()
{
    let endpoints = [endpoint(0, 1), endpoint(0, 2)];
    for identity in [None, Some(json!({"userStatus":{"userTier":{"name":{}}}}))]
    {
        let mut calls = Vec::new();
        let quota = probe(&endpoints, Instant::now() + Duration::from_secs(5), |ep, rpc, _| {
            calls.push((ep.port, rpc));
            match (ep.port, rpc)
            {
                (1, Rpc::Summary) => Some(json!({"groups":[{"buckets":[{"window":"weekly","disabled":true}]}]})),
                (2, Rpc::Summary) => Some(json!({"response":{"groups":[{"displayName":"Gemini Models",
                    "buckets":[{"window":"weekly","remainingFraction":0,"resetTime":"2026-11-27T00:00:00Z"}]}]}})),
                (2, Rpc::UserStatus) => identity.clone(),
                _ => panic!("Unexpected RPC"),
            }
        }).unwrap();
        assert_eq!(calls, [(1, Rpc::Summary), (2, Rpc::Summary), (2, Rpc::UserStatus)]);
        assert_eq!(quota.plan, None);
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].group, "Gemini");
        assert_eq!(quota.windows[0].remaining, 0.0);
        assert!(quota.windows[0].resets_at.is_some());
    }
}

#[test]
fn model_configs_work_without_user_status_and_keep_source_priority()
{
    let endpoints = [endpoint(2, 30), endpoint(1, 20), endpoint(0, 10)];
    let mut calls = Vec::new();
    let quota = probe(&endpoints, Instant::now() + Duration::from_secs(5), |ep, rpc, _| {
        assert_eq!(ep.port, 10);
        calls.push(rpc);
        match rpc
        {
            Rpc::Summary => Some(json!({})),
            Rpc::UserStatus => None,
            Rpc::ModelConfigs => Some(json!({"clientModelConfigs":[
                {"modelOrAlias":{"model":"MODEL_PLACEHOLDER_M9"},"quotaInfo":{"remainingFraction":0}},
                {"modelOrAlias":{"model":"MODEL_CHAT_42"},"quotaInfo":{"remainingFraction":0.75}}
            ]})),
        }
    }).unwrap();
    assert_eq!(calls, [Rpc::Summary, Rpc::UserStatus, Rpc::ModelConfigs]);
    assert_eq!(quota.plan, None);
    assert_eq!(quota.windows.len(), 1);
    assert_eq!(quota.windows[0].group, "Claude/GPT");
    assert_eq!(quota.windows[0].remaining, 75.0);
}

#[test]
fn falls_through_to_the_ide_and_keeps_its_legacy_plan()
{
    let endpoints = [endpoint(0, 10), endpoint(2, 20)];
    let quota = probe(&endpoints, Instant::now() + Duration::from_secs(5), |ep, rpc, _| {
        if ep.port != 20 || rpc != Rpc::UserStatus { return None; }
        Some(json!({"userStatus":{"userTier":{"name":" "},"planStatus":{"planInfo":{"displayName":" Pro "}},
            "cascadeModelConfigData":{"clientModelConfigs":[
                {"modelOrAlias":{"model":"GEMINI_FLASH"},"quotaInfo":{"remainingFraction":0.25}}
            ]}}}))
    }).unwrap();
    assert_eq!(quota.plan.as_deref(), Some("Pro"));
    assert_eq!(quota.windows[0].group, "Gemini Flash");
    assert_eq!(quota.windows[0].remaining, 25.0);
}

#[test]
fn windows_discovery_preserves_hub_and_extension_ports_and_their_own_tokens()
{
    let script = concat!(r#"
function Get-CimInstance
{
    [pscustomobject]@{Name='language_server.exe';ProcessId=17;CommandLine='C:\antigravity\language_server.exe --app_data_dir antigravity --csrf_token main-test --hub-port=1122 --extension_server_port 3344 --extension_server_csrf_token ext-test'}
    [pscustomobject]@{Name='agy.exe';ProcessId=18;CommandLine='C:\tools\agy.exe --hub --hub-port 5566'}
}
function Get-NetTCPConnection
{
    [pscustomobject]@{OwningProcess=17;LocalPort=2233}
}
"#, include_str!("../antigravity_endpoints.ps1"));
    let endpoints: Vec<Endpoint> = powershell_json(script, &[], Duration::from_secs(8), &AppLog::default()).unwrap();
    let actual: Vec<_> = endpoints.iter().map(|ep| (ep.rank, ep.port, ep.secure, ep.csrf.as_str())).collect();
    assert_eq!(actual, [(0, 1122, true, "main-test"), (0, 1122, false, "main-test"),
        (0, 2233, true, "main-test"), (0, 2233, false, "main-test"), (0, 3344, false, "ext-test")]);
}

#[test]
#[ignore = "requires a running Antigravity service with readable quota"]
fn live_panel_quota()
{
    use crate::{quota::{QuotaCache, QuotaRefresh}, settings::{QuotaSettings, ToolSelection}, sources::Sources};

    let cache = QuotaCache::default();
    let settings = QuotaSettings { tools: ToolSelection { antigravity: true, codex: false, dsh: false },
        ..QuotaSettings::default() };
    let snapshot = cache.read(&Sources::discover(), settings, QuotaRefresh::IfStale);
    let cached = cache.cached(settings.tools).unwrap();
    assert_eq!(snapshot.tools.len(), 1);
    let quota = &cached.tools[0];
    assert_eq!(quota.tool, "antigravity");
    assert!(!quota.windows.is_empty());
    for window in &quota.windows
    {
        assert!((0.0..=100.0).contains(&window.remaining));
        println!("{} ({:?} minutes), reset available: {}", window.group, window.minutes, window.resets_at.is_some());
    }
}
