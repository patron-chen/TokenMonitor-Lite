use std::{
    env, fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};
use tauri::{Url, WebviewWindow};

const MAX_BYPASS_LENGTH: usize = 2000;
static STORE: OnceLock<ProxyStore> = OnceLock::new();

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ProxySettings
{
    pub(crate) proxy_url: String,
    pub(crate) no_proxy: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ProxyError
{
    InvalidUrl,
    UnsupportedScheme,
    CredentialsUnsupported,
    InvalidBypass,
    BypassTooLong,
    SaveFailed,
}

fn normalize_url(value: &str) -> Result<String, ProxyError>
{
    let value = value.trim();
    if value.is_empty() { return Ok(String::new()); }
    if value.chars().any(char::is_whitespace) { return Err(ProxyError::InvalidUrl); }
    let url = if value.contains("://") { Url::parse(value) } else { Url::parse(&format!("http://{value}")) }
        .map_err(|_| ProxyError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
    {
        return Err(ProxyError::UnsupportedScheme);
    }
    if !url.username().is_empty() || url.password().is_some() { return Err(ProxyError::CredentialsUnsupported); }
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() || url.port() == Some(0)
    {
        return Err(ProxyError::InvalidUrl);
    }
    let host = url.host_str().ok_or(ProxyError::InvalidUrl)?;
    let port = url.port().map_or_else(String::new, |port| format!(":{port}"));
    Ok(format!("{}://{host}{port}", url.scheme()))
}

fn bypass_entry(value: &str) -> Option<String>
{
    if value == "*" { return Some(value.into()); }
    if let Some((address, length)) = value.split_once('/')
    {
        if !length.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
        let prefix = length.parse::<u32>().ok()?;
        // Clear host bits when saving. A /0 prefix has no network bits.
        let network: IpAddr = match address.parse::<IpAddr>().ok()?
        {
            IpAddr::V4(address) if prefix <= 32 =>
            {
                let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
                Ipv4Addr::from(u32::from(address) & mask).into()
            }
            IpAddr::V6(address) if prefix <= 128 =>
            {
                let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
                Ipv6Addr::from(u128::from(address) & mask).into()
            }
            _ => return None,
        };
        return Some(format!("{network}/{prefix}"));
    }
    if let Ok(address) = value.parse::<Ipv6Addr>() { return Some(format!("[{address}]")); }
    let (host, port) = if let Some(bare) = value.strip_prefix('[')
    {
        let (address, suffix) = bare.split_once(']')?;
        let address = address.parse::<Ipv6Addr>().ok()?;
        let port = if suffix.is_empty() { None } else { Some(suffix.strip_prefix(':')?) };
        (format!("[{address}]"), port)
    }
    else
    {
        let (name, port) = value.rsplit_once(':').map_or((value, None), |(name, port)| (name, Some(port)));
        let name = name.strip_prefix("*.").or_else(|| name.strip_prefix('.')).unwrap_or(name).trim_end_matches('.');
        let valid_name = name.split('.').all(|label| {
            !label.is_empty() && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        });
        if !valid_name || (name.parse::<Ipv4Addr>().is_err() && Url::parse(&format!("http://{name}")).ok()?.domain().is_none())
        {
            return None;
        }
        (name.to_ascii_lowercase(), port)
    };
    if let Some(port) = port
    {
        let number = port.parse::<u16>().ok()?;
        if number == 0 || !port.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
        return Some(format!("{host}:{number}"));
    }
    Some(host)
}

impl ProxySettings
{
    fn normalize(self) -> Result<Self, ProxyError>
    {
        let proxy_url = normalize_url(&self.proxy_url)?;
        let mut rules = Vec::new();
        for value in self.no_proxy.split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';')).filter(|value| !value.is_empty())
        {
            let entry = bypass_entry(value).ok_or(ProxyError::InvalidBypass)?;
            if !rules.contains(&entry) { rules.push(entry); }
        }
        let no_proxy = if rules.iter().any(|rule| rule == "*") { "*".into() } else { rules.join(",") };
        if no_proxy.len() > MAX_BYPASS_LENGTH { return Err(ProxyError::BypassTooLong); }
        Ok(Self { proxy_url, no_proxy })
    }
}

struct ProxyStore
{
    file: PathBuf,
    settings: Mutex<ProxySettings>,
}

impl ProxyStore
{
    fn load(root: &Path) -> Self
    {
        let file = root.join("proxy.json");
        let settings = fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<ProxySettings>(&bytes).ok())
            .and_then(|settings| settings.normalize().ok()).unwrap_or_default();
        Self { file, settings: Mutex::new(settings) }
    }

    fn get(&self) -> ProxySettings
    {
        self.settings.lock().expect("proxy settings").clone()
    }

    fn save(&self, next: ProxySettings) -> Result<ProxySettings, ProxyError>
    {
        let next = next.normalize()?;
        let mut saved = self.settings.lock().expect("proxy settings");
        if *saved != next
        {
            let temporary = self.file.with_extension("tmp");
            let encoded = serde_json::to_vec(&next).map_err(|_| ProxyError::SaveFailed)?;
            fs::write(&temporary, encoded).and_then(|_| fs::rename(&temporary, &self.file)).map_err(|_| ProxyError::SaveFailed)?;
            *saved = next.clone();
        }
        Ok(next)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum ProxySource
{
    Custom,
    LaunchArgument,
    Environment,
    Windows,
}

#[derive(Serialize)]
pub(crate) struct ProxyStatus
{
    #[serde(flatten)]
    settings: ProxySettings,
    source: ProxySource,
}

impl ProxyStatus
{
    fn new(settings: ProxySettings) -> Self
    {
        let configured = |key| env::var(key).is_ok_and(|value| !value.trim().is_empty());
        let source = if !settings.proxy_url.is_empty() { ProxySource::Custom }
            else if configured("TOKEN_MONITOR_LITE_PROXY_SERVER") { ProxySource::LaunchArgument }
            else if ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"].into_iter().any(configured) { ProxySource::Environment }
            else { ProxySource::Windows };
        Self { settings, source }
    }
}

pub(crate) fn initialize(root: &Path)
{
    STORE.get_or_init(|| ProxyStore::load(root));
}

pub(crate) fn current() -> Option<ProxySettings>
{
    STORE.get().map(ProxyStore::get)
}

#[tauri::command]
pub(crate) fn get_proxy_settings(window: WebviewWindow) -> Result<ProxyStatus, ProxyError>
{
    if window.label() != crate::WINDOW { return Err(ProxyError::SaveFailed); }
    current().map(ProxyStatus::new).ok_or(ProxyError::SaveFailed)
}

#[tauri::command]
pub(crate) fn set_proxy_settings(window: WebviewWindow, settings: ProxySettings) -> Result<ProxyStatus, ProxyError>
{
    if window.label() != crate::WINDOW { return Err(ProxyError::SaveFailed); }
    STORE.get().ok_or(ProxyError::SaveFailed)?.save(settings).map(ProxyStatus::new)
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn normalizes_supported_proxy_urls_and_rejects_credentials()
    {
        for (value, expected) in [
            (" 127.0.0.1:7890 ", "http://127.0.0.1:7890"),
            ("https://proxy.example:443/", "https://proxy.example"),
            ("socks5://127.0.0.1:1080", "socks5://127.0.0.1:1080"),
            ("socks5h://[::1]:1080", "socks5h://[::1]:1080"),
            ("  ", ""),
        ]
        {
            assert_eq!(normalize_url(value).as_deref(), Ok(expected));
        }
        assert_eq!(normalize_url("ftp://proxy.example"), Err(ProxyError::UnsupportedScheme));
        assert_eq!(normalize_url("http://user:pass@proxy.example"), Err(ProxyError::CredentialsUnsupported));
        for invalid in ["https://", "http://proxy.example/path", "proxy.example:0", "http://proxy.example?q=1"]
        {
            assert_eq!(normalize_url(invalid), Err(ProxyError::InvalidUrl));
        }
    }

    #[test]
    fn canonicalizes_domains_and_ip_ports_in_the_bypass_list()
    {
        let no_proxy = "*.EXAMPLE.com; .example.com  10.0.0.1:0080, [2001:0db8::1]:443, ::1";
        let settings = ProxySettings { no_proxy: no_proxy.into(), ..ProxySettings::default() };
        assert_eq!(settings.normalize().unwrap().no_proxy, "example.com,10.0.0.1:80,[2001:db8::1]:443,[::1]");
        for invalid in ["https://example.com", "<local>", "example.*", "example.com:65536", "10.1.2"]
        {
            assert_eq!(bypass_entry(invalid), None);
        }
    }

    #[test]
    fn normalizes_cidr_and_rejects_invalid_prefixes()
    {
        for (input, expected) in [
            ("10.4.3.2/8", "10.0.0.0/8"),
            ("192.0.2.55/32", "192.0.2.55/32"),
            ("10.4.3.2/0", "0.0.0.0/0"),
            ("2001:0db8:12::9/32", "2001:db8::/32"),
            ("2001:db8::9/128", "2001:db8::9/128"),
            ("2001:db8::9/0", "::/0"),
        ]
        {
            assert_eq!(bypass_entry(input).as_deref(), Some(expected));
        }
        for invalid in ["10.0.0.0/33", "2001:db8::/129", "10.0.0.0/", "10.0.0.0/-1", "10.0.0.0/+8", "example.com/8"]
        {
            assert_eq!(bypass_entry(invalid), None);
        }
    }

    #[test]
    fn saves_proxy_settings_and_preserves_the_previous_value_on_invalid_input()
    {
        let root = env::temp_dir().join(format!("token-monitor-lite-proxy-settings-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let store = ProxyStore::load(&root);
        let saved = store.save(ProxySettings {
            proxy_url: "localhost:7890".into(), no_proxy: ".example.com,10.1.2.3/8".into(),
        }).unwrap();
        assert_eq!(saved.no_proxy, "example.com,10.0.0.0/8");
        assert_eq!(store.save(ProxySettings { proxy_url: "http://user:pass@localhost".into(), ..ProxySettings::default() }),
            Err(ProxyError::CredentialsUnsupported));
        assert_eq!(ProxyStore::load(&root).get(), saved);
        store.save(ProxySettings::default()).unwrap();
        assert_eq!(ProxyStore::load(&root).get(), ProxySettings::default());
        fs::remove_file(root.join("proxy.json")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
