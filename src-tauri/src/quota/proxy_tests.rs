use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use serde_json::{json, Value};

use crate::app_log::{AppLog, LOG_FILE};
use super::{parse_online_codex, powershell_json, QUOTA_SCRIPT, REMOTE_HELPER_TIMEOUT};

#[test]
fn only_a_manual_panel_request_starts_another_codex_usage_query()
{
    use super::{QuotaCache, QuotaRefresh};
    use crate::{settings::{QuotaSettings, ToolSelection}, sources::Sources};

    let root = std::env::temp_dir().join(format!("token-monitor-manual-quota-{}", std::process::id()));
    let cache = QuotaCache { log: std::sync::Arc::new(AppLog::new(&root)), ..QuotaCache::default() };
    let tools = ToolSelection { codex: true, antigravity: false, dsh: false };
    let settings = QuotaSettings { tools, ..QuotaSettings::default() };
    // Keep all source paths in the fixture. Missing auth stops each real helper before HTTP.
    let mut sources = Sources::from_home(Some(root.clone()));
    sources.home_override = None;
    cache.read(&sources, settings, QuotaRefresh::Panel);
    cache.read(&sources, settings, QuotaRefresh::IfStale);
    cache.read(&sources, settings, QuotaRefresh::Manual);
    let disabled = ToolSelection { codex: false, ..tools };
    cache.read(&sources, QuotaSettings { tools: disabled, ..settings }, QuotaRefresh::Manual);

    let log = root.join("logs").join(LOG_FILE);
    let trace = fs::read_to_string(&log).unwrap();
    assert_eq!(trace.matches("codex_usage trigger=manual auth result=file_missing").count(), 1);
    assert_eq!(trace.matches("banked_reset auth result=file_missing").count(), 2);
    assert!(!trace.contains("codex_usage trigger=panel"));
    fs::remove_file(log).unwrap();
    fs::remove_dir(root.join("logs")).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
fn slow_remote_quota_uses_shortcut_then_environment_and_respects_bypass()
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = format!("http://{address}");
    let origin = format!("{proxy}/direct");
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        let mut delayed = false;
        for client in listener.incoming()
        {
            let mut client = client.unwrap();
            let mut headers = BufReader::new(&client).lines().map(Result::unwrap);
            // An empty connection stops the fixture even if a proxy request failed.
            let Some(first) = headers.next() else { break };
            let headers: Vec<_> = headers.take_while(|line| !line.is_empty()).collect();
            let target = first.split_whitespace().nth(1).unwrap();
            let authorized = |value: &str| headers.iter().any(|h| h.eq_ignore_ascii_case(value));
            // The old shared helper was killed at 13s, even with a legitimate response in flight.
            if target == "http://quota.invalid/resets" && !delayed
            {
                delayed = true;
                thread::sleep(Duration::from_secs(14));
            }
            let body = match target
            {
                "http://quota.invalid/usage" if authorized("Authorization: Bearer codex-fixture")
                    && authorized("ChatGPT-Account-Id: quota-account-fixture") =>
                    json!({"plan_type":"prolite","rate_limit":{"primary_window":{
                        "used_percent":25,"limit_window_seconds":604800,"reset_at":1_800_000_000}}}),
                "http://quota.invalid/resets" if authorized("Authorization: Bearer codex-fixture") =>
                    json!({"available_count":3, "credits":[]}),
                "http://quota.invalid/balance" if authorized("Authorization: Bearer deepseek-fixture") =>
                    json!({"balance_infos":[{"total_balance":"12.50"}]}),
                _ => json!({"target":target}),
            }.to_string();
            // Observe HTTPS proxy selection without opening a real upstream connection.
            let status = if first.starts_with("CONNECT ") { "403 Forbidden" } else { "200 OK" };
            write!(client, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            requests.push(first);
        }
        requests
    });

    let root = std::env::temp_dir().join(format!("token-monitor-proxy-test-{}", std::process::id()));
    let auth = root.join("auth.json");
    let log = AppLog::new(&root);
    fs::write(&auth, r#"{"tokens":{"access_token":"codex-fixture","account_id":"quota-account-fixture"}}"#).unwrap();
    let helper = QUOTA_SCRIPT
        .replace("https://chatgpt.com/backend-api/wham/usage", "http://quota.invalid/usage")
        .replace("https://chatgpt.com/backend-api/wham/rate-limit-reset-credits", "http://quota.invalid/resets")
        .replace("https://api.deepseek.com/user/balance", "http://quota.invalid/balance");
    let probes = r#"
$null = Get-QuotaResponse 'https://quota.invalid/' @{}
$loopback = Get-LiteProxy $env:QUOTA_TEST_ORIGIN
$env:NO_PROXY = '.other.invalid'
$shortcutBypass = Get-LiteProxy 'https://sub.other.invalid/'
$env:HTTP_PROXY = $env:TOKEN_MONITOR_LITE_PROXY_SERVER
$env:HTTPS_PROXY = $env:TOKEN_MONITOR_LITE_PROXY_SERVER
$env:TOKEN_MONITOR_LITE_PROXY_SERVER = $null
$env:ALL_PROXY = $env:HTTP_PROXY
$env:HTTP_PROXY = $null
$fallback = Get-QuotaResponse 'http://quota.invalid/fallback' @{}
$env:NO_PROXY = '.other.invalid,127.0.0.1'
$direct = Get-QuotaResponse $env:QUOTA_TEST_ORIGIN @{}
$bypassed = Get-LiteProxy 'https://sub.other.invalid/path'
$unrelated = Get-LiteProxy 'https://notother.invalid/'
$env:HTTP_PROXY = 'http://user:proxy-secret-fixture@' + ([uri]$env:ALL_PROXY).Authority
$curl = Get-QuotaResponse 'http://quota.invalid/resets' @{Authorization='Bearer codex-fixture'} ${function:Write-BankedResetTrace}
ConvertTo-Json -Depth 10 -InputObject @{ providers = $providers; fallback = $fallback; direct = $direct; loopback = $loopback; shortcutBypass = $shortcutBypass; bypassed = $bypassed; unrelated = $unrelated; curl = $curl }
"#;
    let script = format!("$providers = . {{\n{helper}\n}} | ConvertFrom-Json\n{probes}");
    let shortcut = crate::network::proxy_server([format!("--proxy-server={proxy}")]).unwrap();
    let result: Result<Value, _> = powershell_json(&script, &[
        (crate::network::PROXY_SERVER_ENV, shortcut.as_ref()),
        ("HTTP_PROXY", "http://127.0.0.1:1".as_ref()), ("HTTPS_PROXY", "http://127.0.0.1:1".as_ref()),
        ("ALL_PROXY", "http://127.0.0.1:1".as_ref()), ("NO_PROXY", "".as_ref()),
        ("QUOTA_TEST_ORIGIN", origin.as_ref()),
        ("TOKEN_MONITOR_LITE_QUOTA_TOOLS", "codex_usage,codex,dsh".as_ref()),
        ("TOKEN_MONITOR_LITE_QUOTA_TRIGGER", "manual".as_ref()),
        ("TOKEN_MONITOR_LITE_CODEX_AUTH", auth.as_os_str()), ("DEEPSEEK_API_KEY", "deepseek-fixture".as_ref()),
    ], REMOTE_HELPER_TIMEOUT, &log);
    fs::remove_file(auth).unwrap();
    drop(TcpStream::connect(address).unwrap());
    let requests = server.join().unwrap();
    let logs = root.join("logs");
    let trace = fs::read_to_string(logs.join(LOG_FILE)).unwrap();
    fs::remove_file(logs.join(LOG_FILE)).unwrap();
    fs::remove_dir(logs).unwrap();
    fs::remove_dir(root).unwrap();

    let result = result.expect("quota helper JSON");
    let quota = parse_online_codex(&result["providers"]["codex_usage"], 1_700_000_000_000).unwrap();
    assert_eq!(quota.plan.as_deref(), Some("Pro 100"));
    assert_eq!(quota.windows.len(), 1);
    assert_eq!(quota.windows[0].minutes, Some(10_080));
    assert_eq!(quota.windows[0].remaining, 75.0);
    assert_eq!(requests.iter().filter(|line| line.starts_with("GET http://quota.invalid/usage ")).count(), 1);
    assert_eq!(result["providers"]["codex_reset"]["available_count"], 3);
    assert_eq!(result["providers"]["deepseek"]["balance_infos"][0]["total_balance"], "12.50");
    assert!(requests.iter().any(|line| line.starts_with("CONNECT quota.invalid:443 ")));
    assert_eq!(result["fallback"]["target"], "http://quota.invalid/fallback");
    assert_eq!(result["direct"]["target"], "/direct");
    assert_eq!(result["loopback"], "DIRECT");
    assert_eq!(result["shortcutBypass"], "DIRECT");
    assert_eq!(result["bypassed"], "DIRECT");
    assert_eq!(result["unrelated"], proxy);
    assert_eq!(result["curl"]["available_count"], 3, "curl reply: {} (requests: {requests:?})", result["curl"]);
    for expected in ["route source=proxy-server", "client=winhttp proxy_scheme=http proxy_loopback=True",
        "winhttp phase=send timeout_ms=20000", "winhttp phase=wait", "http status=200",
        "response count_field=available_count count=3", "route source=HTTP_PROXY",
        "transport client=curl", "curl exit=0 http=200", "codex_usage trigger=manual route source=proxy-server",
        "codex_usage trigger=manual http status=200"]
    {
        assert!(trace.contains(expected), "missing {expected} in {trace}");
    }
    for private in ["codex-fixture", "quota-account-fixture", "deepseek-fixture", "proxy-secret-fixture", "quota.invalid", "127.0.0.1", "/resets"]
    {
        assert!(!trace.contains(private));
    }
}

#[test]
fn winhttp_ends_a_slow_exchange_even_when_body_bytes_keep_arriving()
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = format!("http://{address}");
    let server = thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        let mut headers = BufReader::new(&client).lines().map_while(Result::ok);
        let Some(first) = headers.next() else { return false };
        assert!(first.starts_with("GET http://quota.invalid/trickle "));
        for line in headers { if line.is_empty() { break; } }
        write!(client, "HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n").unwrap();
        // A per-packet receive timer alone would accept all 20 bytes over four seconds.
        for _ in 0..20
        {
            if client.write_all(b"x").is_err() { break; }
            thread::sleep(Duration::from_millis(200));
        }
        true
    });
    let script = concat!(include_str!("../../../scripts/windows-http.ps1"), r#"
$watch = [Diagnostics.Stopwatch]::StartNew()
try { $null = Invoke-LiteHttpRequest -Uri 'http://quota.invalid/trickle' -TimeoutMs 1000; $completed = $true }
catch { $completed = $false }
ConvertTo-Json -InputObject @{completed=$completed; elapsed=$watch.ElapsedMilliseconds}
"#);
    let result: Result<Value, _> = powershell_json(script, &[
        (crate::network::PROXY_SERVER_ENV, proxy.as_ref()), ("NO_PROXY", "".as_ref()),
    ], Duration::from_secs(10), &AppLog::default());
    let _ = TcpStream::connect(address);
    assert!(server.join().unwrap());
    let result = result.unwrap();
    assert_eq!(result["completed"], false);
    assert!((700..3500).contains(&result["elapsed"].as_u64().unwrap()), "{result}");
}

#[test]
fn banked_reset_failures_are_distinguishable_without_logging_account_or_response_text()
{
    let root = std::env::temp_dir().join(format!("token-monitor-banked-test-{}", std::process::id()));
    let auth = root.join("auth.json");
    let log = AppLog::new(&root);
    let probes = r#"
function Invoke-LiteHttpRequest([uri]$Uri, $Headers, [int]$TimeoutMs, [scriptblock]$Trace)
{
    if ($script:status -eq 0) { throw 'private-error-fixture' }
    return @{Status=$script:status; Text=$script:body}
}
$file = $env:TOKEN_MONITOR_LITE_CODEX_AUTH
[Console]::Error.WriteLine('private-untagged-stderr')
$env:TOKEN_MONITOR_LITE_CODEX_AUTH = $file + '.missing'
$null = Get-CodexBankedReset
$env:TOKEN_MONITOR_LITE_CODEX_AUTH = $file
[IO.File]::WriteAllText($file, '{private-malformed-auth')
$null = Get-CodexBankedReset
[IO.File]::WriteAllText($file, '{"OPENAI_API_KEY":"private-key-fixture"}')
$null = Get-CodexBankedReset
[IO.File]::WriteAllText($file, '{"tokens":{"access_token":"private-token","account_id":"private-account"}}')
foreach ($script:status in 0,401,403,429,503) { $null = Get-CodexBankedReset }
$script:status = 200
foreach ($script:body in '', '{private-malformed-body', '{"message":"private-reply"}', '{"available_count":null}', '{"available_count":"private-value"}', '{"availableCount":0}')
{
    $null = Get-CodexBankedReset
}
$script:status = 403
$usage = Get-CodexUsage
if ($usage.error -ne 'no_usage_response') { throw 'Unexpected Codex usage result' }
Write-Output 'true'
"#;
    let script = format!(". {{\n{QUOTA_SCRIPT}\n}} | Out-Null\n{probes}");
    let result: Result<bool, _> = powershell_json(&script, &[
        ("TOKEN_MONITOR_LITE_CODEX_AUTH", auth.as_os_str()),
        ("TOKEN_MONITOR_LITE_QUOTA_TOOLS", "".as_ref()),
        ("TOKEN_MONITOR_LITE_QUOTA_TRIGGER", "startup".as_ref()),
    ], Duration::from_secs(12), &log);
    let logs = root.join("logs");
    let trace = fs::read_to_string(logs.join(LOG_FILE)).unwrap();
    fs::remove_file(&auth).unwrap();
    fs::remove_file(logs.join(LOG_FILE)).unwrap();
    fs::remove_dir(logs).unwrap();
    fs::remove_dir(root).unwrap();
    assert!(result.unwrap());
    for expected in ["auth result=file_missing", "failure stage=auth_json", "auth result=no_access_token api_key_present=True",
        "auth result=ready account_header=True", "failure stage=request hresult=0x", "http status=401", "http status=403",
        "http status=429", "http status=503", "response result=empty_body", "failure stage=response_json",
        "count_field=missing count=unusable", "count_field=null count=unusable",
        "count_field=available_count count=unusable", "count_field=availableCount count=0",
        "codex_usage trigger=startup http status=403"]
    {
        assert!(trace.contains(expected), "missing {expected} in {trace}");
    }
    assert!(!trace.contains("private-") && !trace.contains(auth.to_str().unwrap()));
}

#[test]
fn socks5_retries_with_proxy_dns_and_preserves_headers_and_json_body()
{
    const PAYLOAD: &str = "{\n  \"message\": \"quoted \\\"value\\\" and path C:\\\\tmp\"\n}";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = format!("socks5://{address}");
    let server = thread::spawn(move || {
        for stream in listener.incoming()
        {
            let mut stream = stream.unwrap();
            let mut greeting = [0; 2];
            if stream.read_exact(&mut greeting).is_err() { continue; }
            if greeting == [0, 0] { break; }
            assert_eq!(greeting[0], 5);
            stream.read_exact(&mut vec![0; greeting[1] as usize]).unwrap();
            stream.write_all(&[5, 0]).unwrap();
            let mut connect = [0; 4];
            // curl can close here when local DNS has no address. Reject locally resolved IPs too.
            if stream.read_exact(&mut connect).is_err() { continue; }
            if connect[3] != 3
            {
                stream.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                continue;
            }
            let mut length = [0];
            stream.read_exact(&mut length).unwrap();
            let mut host = vec![0; length[0] as usize];
            stream.read_exact(&mut host).unwrap();
            stream.read_exact(&mut [0; 2]).unwrap();
            assert_eq!(host, b"quota.invalid");
            stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80]).unwrap();
            let mut request = BufReader::new(&stream);
            let mut first = String::new();
            request.read_line(&mut first).unwrap();
            let headers: Vec<_> = request.by_ref().lines().map(Result::unwrap).take_while(|line| !line.is_empty()).collect();
            let authorized = headers.iter().any(|line| line.eq_ignore_ascii_case("Authorization: Bearer socks-fixture"));
            let length: usize = headers.iter().filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("Content-Length")).map_or(0, |(_, value)| value.trim().parse().unwrap());
            let mut payload = vec![0; length];
            request.read_exact(&mut payload).unwrap();
            let body = json!({"authorized":authorized,"got":first.starts_with("GET /resets "),
                "posted":first.starts_with("POST /resets ") && payload == PAYLOAD.as_bytes()}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let script = format!(". {{\n{QUOTA_SCRIPT}\n}} | Out-Null\n\
        $get = Get-QuotaResponse 'http://quota.invalid/resets' @{{Authorization='Bearer socks-fixture'}}\n\
        $reply = Invoke-LiteHttpRequest -Uri 'http://quota.invalid/resets' -Method POST -Body $env:QUOTA_TEST_BODY \
            -Headers @{{Authorization='Bearer socks-fixture';'Content-Type'='application/json'}}\n\
        ConvertTo-Json -InputObject @{{get=$get; post=(ConvertFrom-Json -InputObject $reply.Text)}}");
    let result: Result<Value, _> = powershell_json(&script, &[
        (crate::network::PROXY_SERVER_ENV, "".as_ref()),
        ("QUOTA_TEST_BODY", PAYLOAD.as_ref()),
        ("HTTP_PROXY", proxy.as_ref()), ("NO_PROXY", "".as_ref()), ("TOKEN_MONITOR_LITE_QUOTA_TOOLS", "".as_ref()),
    ], Duration::from_secs(15), &AppLog::default());
    TcpStream::connect(address).unwrap().write_all(&[0, 0]).unwrap();
    server.join().unwrap();
    assert_eq!(result.unwrap(), json!({
        "get":{"authorized":true,"got":true,"posted":false},
        "post":{"authorized":true,"got":false,"posted":true}
    }));
}
