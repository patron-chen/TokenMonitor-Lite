use std::{ffi::c_void, ptr::{self, NonNull}, time::{Duration, Instant}};

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::Networking::WinHttp::*,
};

use super::{antigravity_quota, powershell_json, ToolQuota};

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
    let deadline = Instant::now() + Duration::from_secs(13);
    let endpoints: Vec<Endpoint> = powershell_json(include_str!("antigravity_endpoints.ps1"), &[], Duration::from_secs(8))?;
    if endpoints.is_empty() { return None; }
    let client = unsafe {
        Handle::new(WinHttpOpen(w!("Token Monitor Lite"), WINHTTP_ACCESS_TYPE_NO_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?
    };
    probe(&endpoints, deadline, |endpoint, rpc, timeout| client.request(endpoint, rpc, timeout))
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
                quota.plan = match rpc
                {
                    // Identity is optional. An empty or failed status must preserve a valid summary.
                    Rpc::Summary =>
                    {
                        let timeout = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(1));
                        if timeout < Duration::from_millis(1) { None }
                        else { request(endpoint, Rpc::UserStatus, timeout).as_ref().and_then(plan) }
                    }
                    Rpc::UserStatus => plan(&data),
                    Rpc::ModelConfigs => None,
                };
                return Some(quota);
            }
        }
    }
    None
}

// WinHTTP owns distinct session, connection and request handles, including on early returns.
struct Handle(NonNull<c_void>);

impl Drop for Handle
{
    fn drop(&mut self) { unsafe { let _ = WinHttpCloseHandle(self.0.as_ptr()); } }
}

impl Handle
{
    fn new(raw: *mut c_void) -> Option<Self> { NonNull::new(raw).map(Self) }

    fn option(&self, name: u32, value: u32) -> Option<()>
    {
        unsafe { WinHttpSetOption(Some(self.0.as_ptr()), name, Some(&value.to_ne_bytes())).ok() }
    }

    fn request(&self, endpoint: &Endpoint, rpc: Rpc, timeout: Duration) -> Option<Value>
    {
        let method = match rpc
        {
            Rpc::Summary => "RetrieveUserQuotaSummary",
            Rpc::UserStatus => "GetUserStatus",
            Rpc::ModelConfigs => "GetCommandModelConfigs",
        };
        let path = HSTRING::from(format!("/exa.language_server_pb.LanguageServerService/{method}"));
        let body: &[u8] = if rpc == Rpc::Summary { br#"{"forceRefresh":true}"# }
            else { br#"{"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"en"}}"# };
        let mut headers = String::from("Content-Type: application/json\r\nConnect-Protocol-Version: 1\r\n");
        if !endpoint.csrf.is_empty()
        {
            if endpoint.csrf.contains(['\r', '\n']) { return None; }
            headers.push_str(&format!("X-Codeium-Csrf-Token: {}\r\n", endpoint.csrf));
        }
        let headers: Vec<u16> = headers.encode_utf16().collect();
        let deadline = Instant::now() + timeout;
        let ms = timeout.as_millis().max(1) as i32;
        unsafe
        {
            let connection = Handle::new(WinHttpConnect(self.0.as_ptr(), w!("127.0.0.1"), endpoint.port, 0))?;
            let flags = if endpoint.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
            let request = Handle::new(WinHttpOpenRequest(connection.0.as_ptr(), w!("POST"), &path,
                PCWSTR::null(), PCWSTR::null(), ptr::null(), flags))?;
            let raw = request.0.as_ptr();
            request.option(WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER)?;
            request.option(WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH)?;
            if endpoint.secure
            {
                // The loopback service presents its own certificate. Keep the exception on this request.
                request.option(WINHTTP_OPTION_SECURITY_FLAGS, SECURITY_FLAG_IGNORE_UNKNOWN_CA | SECURITY_FLAG_IGNORE_CERT_CN_INVALID)?;
            }
            WinHttpSetTimeouts(raw, ms.min(800), ms.min(800), ms.min(1000), ms).ok()?;
            WinHttpSendRequest(raw, Some(&headers), Some(body.as_ptr().cast()), body.len() as u32, body.len() as u32, 0).ok()?;
            WinHttpReceiveResponse(raw, ptr::null_mut()).ok()?;
            let mut status = 0_u32;
            let mut size = size_of::<u32>() as u32;
            WinHttpQueryHeaders(raw, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(),
                Some((&raw mut status).cast()), &mut size, ptr::null_mut()).ok()?;
            if status != 200 { return None; }

            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 8192];
            while Instant::now() < deadline
            {
                let mut read = 0;
                WinHttpReadData(raw, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read).ok()?;
                if read == 0 { return serde_json::from_slice(&bytes).ok(); }
                bytes.extend_from_slice(&buffer[..read as usize]);
            }
            None
        }
    }
}
