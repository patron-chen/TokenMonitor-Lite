use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::network::{with_http_worker, HttpWorker};
use super::{antigravity_quota, ToolQuota};

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
struct Endpoint
{
    rank: u8,
    port: u16,
    secure: bool,
    csrf: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Rpc { Summary, UserStatus, ModelConfigs }

pub(super) fn read() -> Option<ToolQuota>
{
    const SCRIPT: &str = concat!(include_str!("../../../scripts/windows-http.ps1"), "\n",
        include_str!("antigravity_endpoints.ps1"), "\nRead-LiteHttpRequests");
    let deadline = Instant::now() + Duration::from_secs(13);
    with_http_worker(SCRIPT, deadline, |worker| {
        let endpoints: Vec<Endpoint> = worker.read_json(Duration::from_secs(8))?;
        probe(&endpoints, deadline, |endpoint, rpc, timeout| request(worker, endpoint, rpc, timeout))
    })
}

fn plan(status: &Value) -> Option<String>
{
    let user = &status["userStatus"];
    let info = &user["planStatus"]["planInfo"];
    [&user["userTier"]["name"], &info["planDisplayName"], &info["displayName"],
        &info["productName"], &info["planName"], &info["planShortName"]].into_iter()
        .filter_map(Value::as_str).map(str::trim).find(|name| !name.is_empty()).map(str::to_owned)
}

fn probe(
    endpoints: &[Endpoint], deadline: Instant, mut request: impl FnMut(&Endpoint, Rpc, Duration) -> Option<Value>,
) -> Option<ToolQuota>
{
    for rank in 0..=2
    {
        let source: Vec<_> = endpoints.iter().filter(|endpoint| endpoint.rank == rank).collect();
        // Prefer grouped quota across this source's endpoints before trying either legacy method.
        for rpc in [Rpc::Summary, Rpc::UserStatus, Rpc::ModelConfigs]
        {
            let now = Instant::now();
            let left = deadline.saturating_duration_since(now);
            let phase_end = now + if rpc == Rpc::ModelConfigs { left } else { left / 2 };
            for (index, endpoint) in source.iter().enumerate()
            {
                let budget = phase_end.saturating_duration_since(Instant::now()) / (source.len() - index) as u32;
                if budget < Duration::from_millis(1) { break; }
                let Some(data) = request(endpoint, rpc, budget.min(Duration::from_millis(2600))) else { continue };
                let (summary, configs) = match rpc
                {
                    Rpc::Summary => (&data, &Value::Null),
                    Rpc::UserStatus => (&Value::Null, &data["userStatus"]["cascadeModelConfigData"]["clientModelConfigs"]),
                    Rpc::ModelConfigs => (&Value::Null, &data["clientModelConfigs"]),
                };
                let Some(mut quota) = antigravity_quota(summary, configs, Utc::now().timestamp_millis()) else { continue };
                let identity = match rpc
                {
                    // Identity is optional. An empty or failed status must preserve a valid summary.
                    Rpc::Summary =>
                    {
                        let timeout = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(1));
                        if timeout < Duration::from_millis(1) { None }
                        else { request(endpoint, Rpc::UserStatus, timeout) }
                    }
                    Rpc::UserStatus => Some(data),
                    Rpc::ModelConfigs => None,
                };
                if let Some(status) = identity
                {
                    quota.plan = plan(&status);
                    let user = &status["userStatus"];
                    quota.account = super::account_tag(user["userId"].as_str().or_else(|| user["email"].as_str()));
                }
                return Some(quota);
            }
        }
    }
    None
}

fn request(worker: &mut HttpWorker, endpoint: &Endpoint, rpc: Rpc, timeout: Duration) -> Option<Value>
{
    let method = match rpc
    {
        Rpc::Summary => "RetrieveUserQuotaSummary",
        Rpc::UserStatus => "GetUserStatus",
        Rpc::ModelConfigs => "GetCommandModelConfigs",
    };
    let scheme = if endpoint.secure { "https" } else { "http" };
    let uri = format!("{scheme}://127.0.0.1:{}/exa.language_server_pb.LanguageServerService/{method}", endpoint.port);
    let body = if rpc == Rpc::Summary { r#"{"forceRefresh":true}"# }
        else { r#"{"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"en"}}"# };
    let mut headers = json!({"Content-Type":"application/json", "Connect-Protocol-Version":"1"});
    if !endpoint.csrf.is_empty() { headers["X-Codeium-Csrf-Token"] = json!(endpoint.csrf); }
    worker.request_json(&json!({
        "Uri": uri, "Method": "POST", "Headers": headers, "Body": body,
        "TimeoutMs": timeout.as_millis().max(1) as u64, "LocalServiceCertificate": endpoint.secure,
    }))
}
