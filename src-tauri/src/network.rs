use std::{
    ffi::OsStr,
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde::de::DeserializeOwned;
use serde_json::Value;
use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};

pub(crate) const PROXY_SERVER_ENV: &str = "TOKEN_MONITOR_LITE_PROXY_SERVER";

pub(crate) fn proxy_server(args: impl IntoIterator<Item = impl AsRef<str>>) -> Option<String>
{
    let mut proxy = None;
    let mut expects_value = false;
    for arg in args
    {
        let arg = arg.as_ref();
        let value = arg.strip_prefix("--proxy-server=").or_else(|| {
            (expects_value && !arg.starts_with('-')).then_some(arg)
        });
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty())
        {
            proxy = Some(value.to_owned());
        }
        expects_value = arg == "--proxy-server";
    }
    proxy
}

pub(crate) fn start_powershell(script: &str, vars: &[(&str, &OsStr)], input: Stdio) -> Option<Child>
{
    let powershell = PathBuf::from(std::env::var_os("SystemRoot")?).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .envs(vars.iter().copied()).stdin(input).stdout(Stdio::piped()).stderr(Stdio::null())
        .creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW).spawn().ok()
}

pub(crate) struct HttpWorker
{
    input: ChildStdin,
    replies: Receiver<String>,
    deadline: Instant,
}

impl HttpWorker
{
    pub(crate) fn read_json<T: DeserializeOwned>(&self, timeout: Duration) -> Option<T>
    {
        let left = self.deadline.saturating_duration_since(Instant::now());
        serde_json::from_str(&self.replies.recv_timeout(left.min(timeout)).ok()?).ok()
    }

    pub(crate) fn request_json(&mut self, request: &Value) -> Option<Value>
    {
        serde_json::to_writer(&mut self.input, request).ok()?;
        self.input.write_all(b"\n").ok()?;
        self.input.flush().ok()?;
        let body: String = self.read_json(self.deadline.saturating_duration_since(Instant::now()))?;
        serde_json::from_str(&body).ok()
    }
}

// Discovery and all subsequent loopback RPCs share one helper and one overall time budget.
pub(crate) fn with_http_worker<T>(script: &str, deadline: Instant, action: impl FnOnce(&mut HttpWorker) -> Option<T>) -> Option<T>
{
    let mut child = start_powershell(script, &[], Stdio::piped())?;
    let input = child.stdin.take().expect("HTTP worker stdin");
    let stdout = child.stdout.take().expect("HTTP worker stdout");
    thread::scope(|scope| {
        let (sender, replies) = mpsc::channel();
        scope.spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok)
            {
                if sender.send(line).is_err() { break; }
            }
        });
        let mut worker = HttpWorker { input, replies, deadline };
        let result = action(&mut worker);
        drop(worker);
        // Also interrupt a Windows call that outlived the discovery or RPC deadline.
        let _ = child.kill();
        let _ = child.wait();
        result
    })
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn accepts_shortcut_and_separate_proxy_arguments()
    {
        let proxy = "http://127.0.0.1:1080";
        assert_eq!(proxy_server(["--proxy-server=http://127.0.0.1:1080", "--open"]).as_deref(), Some(proxy));
        assert_eq!(proxy_server(["--open", "--proxy-server", proxy]).as_deref(), Some(proxy));
        assert_eq!(proxy_server(["--proxy-server", "--open", "--proxy-server= "]), None);
        assert_eq!(proxy_server(["--open"]), None);
    }
}
