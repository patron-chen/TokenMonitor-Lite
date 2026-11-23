use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use chrono::Utc;
use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::{app_log::AppLog, settings::PreferencesStore, WINDOW};

const RELEASES: &str = "https://github.com/patron-chen/TokenMonitor-Lite/releases";
const DOWNLOAD: &str = concat!(
    include_str!("../../scripts/windows-http.ps1"), "\n", include_str!("update_download.ps1")
);
const INSTALL: &str = concat!(include_str!("update_windows.ps1"), "\nInstall-LiteUpdate");
const VERIFY: &str = concat!(include_str!("update_windows.ps1"), "\nTest-LiteUpdateArchive");
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const HELPER_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Candidate
{
    version: Version,
    sha256: String,
}

#[derive(Deserialize)]
struct Manifest
{
    version: String,
    sha256: String,
}

fn newer_release(current: &Version, bytes: &[u8]) -> Result<Option<Candidate>, Failure>
{
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|_| Failure::InvalidManifest)?;
    let version = Version::parse(&manifest.version).map_err(|_| Failure::InvalidManifest)?;
    if manifest.sha256.len() != 64 || !manifest.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Failure::InvalidManifest);
    }
    Ok((version.pre.is_empty() && version.cmp_precedence(current).is_gt()).then(|| Candidate {
        version, sha256: manifest.sha256.to_ascii_lowercase(),
    }))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Phase
{
    #[default]
    Idle,
    Checking,
    Current,
    Available,
    Downloading,
    Ready,
    Installing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Failure
{
    Network,
    MissingManifest,
    InvalidManifest,
    Download,
    InvalidArchive,
    Storage,
    InstallDirectory,
    InstallFailed,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum InstallResult
{
    Updated,
    InstallFailed,
    RollbackFailed,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatus
{
    version: String,
    available_version: Option<String>,
    checked_at: Option<i64>,
    phase: Phase,
    failure: Option<Failure>,
    last_install: Option<InstallResult>,
    can_install: bool,
}

#[derive(Default)]
struct State
{
    phase: Phase,
    checked_at: Option<i64>,
    candidate: Option<Candidate>,
    failure: Option<Failure>,
}

pub(crate) struct Updates
{
    version: Version,
    data: PathBuf,
    exe: Option<PathBuf>,
    last_install: Option<InstallResult>,
    state: Mutex<State>,
}

impl Updates
{
    pub(crate) fn new(app: &AppHandle, root: &Path) -> Self
    {
        let data = root.join("updates");
        let exe = std::env::current_exe().ok().filter(|path| {
            cfg!(all(windows, not(debug_assertions), feature = "custom-protocol")) &&
                path.file_name().is_some_and(|name| !name.eq_ignore_ascii_case("tokscale.exe")) &&
                path.with_file_name("tokscale.exe").is_file()
        });
        let result = data.join("result");
        let last_install = match fs::read_to_string(&result).as_deref()
        {
            Ok("updated") => Some(InstallResult::Updated),
            Ok("installFailed") => Some(InstallResult::InstallFailed),
            Ok("rollbackFailed") => Some(InstallResult::RollbackFailed),
            _ => None,
        };
        if let Some(outcome) = last_install
        {
            let _ = fs::remove_file(result);
            if matches!(outcome, InstallResult::Updated)
            {
                let _ = fs::remove_file(data.join("pending.zip"));
            }
            app.state::<Arc<AppLog>>().write(format_args!("update install result={outcome:?}"));
        }
        Self { version: app.package_info().version.clone(), data, exe, last_install, state: Mutex::default() }
    }

    fn status(&self) -> UpdateStatus
    {
        let state = self.state.lock().expect("update state");
        UpdateStatus {
            version: self.version.to_string(), available_version: state.candidate.as_ref().map(|next| next.version.to_string()),
            checked_at: state.checked_at, phase: state.phase, failure: state.failure, last_install: self.last_install,
            can_install: self.exe.is_some(),
        }
    }

    fn publish(&self, app: &AppHandle)
    {
        let _ = app.emit_to(WINDOW, "update-status-changed", self.status());
    }

    fn fail(&self, app: &AppHandle, phase: Phase, failure: Failure)
    {
        {
            let mut state = self.state.lock().expect("update state");
            state.phase = phase;
            state.failure = Some(failure);
        }
        app.state::<Arc<AppLog>>().write(format_args!("update result={failure:?}"));
        self.publish(app);
    }

    fn latest(&self) -> Result<Option<Candidate>, Failure>
    {
        fs::create_dir_all(&self.data).map_err(|_| Failure::Storage)?;
        let path = self.data.join("manifest.download");
        let status = http_status(&format!("{RELEASES}/latest/download/update.json"), &path, 8192, 15_000);
        let result = match status
        {
            Some(200) => fs::read(&path).map_err(|_| Failure::Storage)
                .and_then(|bytes| newer_release(&self.version, &bytes)),
            Some(404) => Err(Failure::MissingManifest),
            _ => Err(Failure::Network),
        };
        let _ = fs::remove_file(path);
        result
    }

    fn run_check(&self, app: &AppHandle, previous: Phase)
    {
        let next = self.latest();
        self.state.lock().expect("update state").checked_at = Some(Utc::now().timestamp_millis());
        match next
        {
            Ok(Some(candidate)) =>
            {
                let mut state = self.state.lock().expect("update state");
                let ready = previous == Phase::Ready && state.candidate.as_ref() == Some(&candidate);
                let download = !ready && self.exe.is_some() && app.state::<Arc<PreferencesStore>>().get().auto_update;
                state.phase = if ready { Phase::Ready } else if download { Phase::Downloading } else { Phase::Available };
                state.candidate = Some(candidate.clone());
                drop(state);
                self.publish(app);
                if download { self.run_download(app, &candidate); }
            }
            Ok(None) =>
            {
                let mut state = self.state.lock().expect("update state");
                state.candidate = None;
                state.phase = Phase::Current;
                drop(state);
                self.publish(app);
            }
            Err(failure) =>
            {
                let fallback = match previous
                {
                    Phase::Available | Phase::Ready => previous,
                    _ => Phase::Idle,
                };
                self.fail(app, fallback, failure);
            }
        }
    }

    fn download(&self, candidate: &Candidate) -> Result<(), Failure>
    {
        let pending = self.data.join("pending.zip");
        if pending.is_file() && verified(&pending, candidate)
        {
            return Ok(());
        }
        let scratch = self.data.join("archive.download");
        let version = &candidate.version;
        let url = format!("{RELEASES}/download/v{version}/Token-Monitor-Lite-{version}-win-x64.zip");
        let result = if http_status(&url, &scratch, 50 * 1024 * 1024, 60_000) != Some(200)
        {
            Err(Failure::Download)
        }
        else if !verified(&scratch, candidate)
        {
            Err(Failure::InvalidArchive)
        }
        else
        {
            fs::rename(&scratch, &pending).map_err(|_| Failure::Storage)
        };
        let _ = fs::remove_file(scratch);
        result
    }

    fn run_download(&self, app: &AppHandle, candidate: &Candidate)
    {
        match self.download(candidate)
        {
            Ok(()) =>
            {
                self.state.lock().expect("update state").phase = Phase::Ready;
                self.publish(app);
            }
            Err(failure) => self.fail(app, Phase::Available, failure),
        }
    }

    fn run_install(&self, app: &AppHandle, exe: &Path, candidate: &Candidate)
    {
        let path = self.data.join("pending.zip");
        let result = self.data.join("result");
        let parent = std::process::id().to_string();
        let reply = powershell_reply(INSTALL, &[
            ("LITE_UPDATE_FILE", path.as_os_str()),
            ("LITE_UPDATE_SHA256", OsStr::new(&candidate.sha256)),
            ("LITE_UPDATE_EXE", exe.as_os_str()),
            ("LITE_UPDATE_RESULT", result.as_os_str()),
            ("LITE_UPDATE_PARENT", OsStr::new(&parent)),
        ], HELPER_TIMEOUT);
        match reply.as_deref()
        {
            Some("ok") =>
            {
                app.state::<Arc<AppLog>>().write(format_args!("update handoff version={}", candidate.version));
                app.exit(0);
            }
            Some("invalidArchive") => self.fail(app, Phase::Available, Failure::InvalidArchive),
            Some("installDirectory") => self.fail(app, Phase::Ready, Failure::InstallDirectory),
            _ => self.fail(app, Phase::Ready, Failure::InstallFailed),
        }
    }
}

fn http_status(url: &str, path: &Path, max_bytes: u32, timeout: u32) -> Option<u16>
{
    powershell_reply(DOWNLOAD, &[
        ("LITE_UPDATE_URL", OsStr::new(url)), ("LITE_UPDATE_FILE", path.as_os_str()),
        ("LITE_UPDATE_MAX_BYTES", OsStr::new(&max_bytes.to_string())),
        ("LITE_UPDATE_TIMEOUT", OsStr::new(&timeout.to_string())),
    ], Duration::from_millis(u64::from(timeout)) + Duration::from_secs(5))?.parse().ok()
}

fn verified(path: &Path, candidate: &Candidate) -> bool
{
    powershell_reply(VERIFY, &[
        ("LITE_UPDATE_FILE", path.as_os_str()), ("LITE_UPDATE_SHA256", OsStr::new(&candidate.sha256)),
    ], HELPER_TIMEOUT).as_deref() == Some("ok")
}

#[cfg(windows)]
fn powershell_reply(script: &str, vars: &[(&str, &OsStr)], timeout: Duration) -> Option<String>
{
    use std::{io::{BufRead, BufReader}, process::Stdio, sync::mpsc};

    let mut child = crate::network::start_powershell(script, vars, Stdio::null(), Stdio::null())?;
    let output = child.stdout.take()?;
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        if BufReader::new(output).read_line(&mut line).is_ok_and(|size| size != 0)
        {
            let _ = send.send(line.trim().to_owned());
        }
    });
    match receive.recv_timeout(timeout)
    {
        // The installer keeps running after acknowledging that it can take over.
        Ok(reply) => Some(reply),
        Err(_) =>
        {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

#[cfg(not(windows))]
fn powershell_reply(_: &str, _: &[(&str, &OsStr)], _: Duration) -> Option<String>
{
    None
}

#[derive(Clone, Copy)]
enum Action
{
    Check,
    Download,
    Install,
}

fn request(app: &AppHandle, action: Action)
{
    let updates = app.state::<Updates>();
    let mut state = updates.state.lock().expect("update state");
    let previous = state.phase;
    let candidate = state.candidate.clone();
    state.phase = match action
    {
        Action::Check if !matches!(previous, Phase::Checking | Phase::Downloading | Phase::Installing) => Phase::Checking,
        Action::Download if previous == Phase::Available && updates.exe.is_some() && candidate.is_some() => Phase::Downloading,
        Action::Install if previous == Phase::Ready && updates.exe.is_some() && candidate.is_some() => Phase::Installing,
        _ => return,
    };
    state.failure = None;
    drop(state);
    updates.publish(app);

    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let updates = app.state::<Updates>();
        match action
        {
            Action::Check => updates.run_check(&app, previous),
            Action::Download => updates.run_download(&app, &candidate.expect("available update")),
            Action::Install => updates.run_install(&app, updates.exe.as_deref().expect("portable executable"),
                &candidate.expect("verified update")),
        }
    });
}

pub(crate) fn check_automatically(app: &AppHandle)
{
    if app.state::<Updates>().exe.is_some() && app.state::<Arc<PreferencesStore>>().get().auto_update
    {
        request(app, Action::Check);
    }
}

pub(crate) fn start(app: AppHandle)
{
    if app.state::<Updates>().exe.is_none() { return; }
    thread::spawn(move || loop {
        check_automatically(&app);
        thread::sleep(CHECK_INTERVAL);
    });
}

#[tauri::command]
pub(crate) fn update_status(app: AppHandle) -> UpdateStatus
{
    app.state::<Updates>().status()
}

#[tauri::command]
pub(crate) fn check_updates(window: WebviewWindow)
{
    if window.label() == WINDOW { request(window.app_handle(), Action::Check); }
}

#[tauri::command]
pub(crate) fn download_update(window: WebviewWindow)
{
    if window.label() == WINDOW { request(window.app_handle(), Action::Download); }
}

#[tauri::command]
pub(crate) fn install_update(window: WebviewWindow)
{
    if window.label() == WINDOW { request(window.app_handle(), Action::Install); }
}

#[tauri::command]
pub(crate) fn open_update_releases(window: WebviewWindow) -> Result<(), &'static str>
{
    if window.label() != WINDOW { return Err("Invalid window."); }
    #[cfg(windows)]
    {
        use std::ptr::null;
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

        let target: Vec<_> = RELEASES.encode_utf16().chain([0]).collect();
        if unsafe { ShellExecuteW(std::ptr::null_mut(), null(), target.as_ptr(), null(), null(), SW_SHOWNORMAL) } as isize > 32
        {
            return Ok(());
        }
    }
    Err("Could not open GitHub Releases.")
}

#[cfg(test)]
mod tests
{
    use super::*;

    fn manifest(version: &str, sha256: &str) -> Vec<u8>
    {
        serde_json::to_vec(&serde_json::json!({ "version": version, "sha256": sha256 })).unwrap()
    }

    #[test]
    fn only_newer_stable_versions_can_be_downloaded()
    {
        let current = Version::parse("0.1.9").unwrap();
        let digest = "A1".repeat(32);
        for same_or_old in ["0.1.8", "0.1.9", "0.1.9+new", "0.1.10-beta.1"]
        {
            assert_eq!(newer_release(&current, &manifest(same_or_old, &digest)), Ok(None));
        }
        assert_eq!(newer_release(&current, &manifest("0.1.10", &digest)), Ok(Some(Candidate {
            version: Version::parse("0.1.10").unwrap(), sha256: digest.to_ascii_lowercase(),
        })));
    }

    #[test]
    fn rejects_invalid_versions_and_missing_or_malformed_checksums()
    {
        let current = Version::parse("0.1.9").unwrap();
        for bytes in [manifest("v0.1.10", &"a".repeat(64)), manifest("0.1.10", &"z".repeat(64)),
            br#"{"version":"0.1.10"}"#.to_vec(), b"not json".to_vec()]
        {
            assert_eq!(newer_release(&current, &bytes), Err(Failure::InvalidManifest));
        }
    }

    #[cfg(windows)]
    #[test]
    fn download_follows_https_redirects_and_reports_a_failed_request()
    {
        let script = concat!(r#"
function Invoke-LiteHttpRequest([uri]$Uri, $OutFile, $MaxBytes, $TimeoutMs)
{
    if ($Uri.Host -eq 'release.example')
    {
        return @{ Status = 302; Location = 'https://assets.example/update.json' }
    }
    if ($Uri.AbsoluteUri -eq 'https://assets.example/update.json' -and $MaxBytes -eq 8192)
    {
        return @{ Status = 200 }
    }
    throw 'Connection failed.'
}
"#, include_str!("update_download.ps1"));
        for (url, expected) in [
            ("https://release.example/update.json", "200"),
            ("https://failed.example/update.json", "0"),
            ("http://release.example/update.json", "0"),
        ]
        {
            let status = powershell_reply(script, &[
                ("LITE_UPDATE_URL", OsStr::new(url)), ("LITE_UPDATE_FILE", OsStr::new("unused")),
                ("LITE_UPDATE_MAX_BYTES", OsStr::new("8192")), ("LITE_UPDATE_TIMEOUT", OsStr::new("1000")),
            ], HELPER_TIMEOUT);
            assert_eq!(status.as_deref(), Some(expected));
        }
    }

    #[cfg(windows)]
    #[test]
    fn verifies_the_package_with_no_hash_cmdlet_available()
    {
        let script = concat!(include_str!("update_windows.ps1"), r#"
function Get-FileHash { throw 'PowerShell module unavailable.' }

Add-Type -AssemblyName System.IO.Compression
$temporary = [IO.Path]::GetTempFileName()
try
{
    $archive = [IO.Compression.ZipArchive]::new([IO.File]::OpenWrite($temporary), 'Create')
    foreach ($name in $script:LitePackageFiles)
    {
        $entry = $archive.CreateEntry($name).Open()
        $entry.WriteByte(1)
        $entry.Dispose()
    }
    $archive.Dispose()
    $algorithm = [Security.Cryptography.SHA256]::Create()
    $digest = [BitConverter]::ToString($algorithm.ComputeHash([IO.File]::ReadAllBytes($temporary))).Replace('-', '')
    $algorithm.Dispose()
    (Open-LiteUpdateArchive $temporary $digest).Dispose()
    $invalid = $false
    try { (Open-LiteUpdateArchive $temporary ('0' * 64)).Dispose() }
    catch { $invalid = $true }
    if ($invalid) { [Console]::WriteLine('ok') }
}
finally { [IO.File]::Delete($temporary) }
"#);
        assert_eq!(powershell_reply(script, &[], HELPER_TIMEOUT).as_deref(), Some("ok"));
    }
}
