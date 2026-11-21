use std::{fmt, fs, io::Write, path::{Path, PathBuf}, sync::Mutex};

use chrono::{DateTime, SecondsFormat, Utc};

pub(crate) const LOG_FILE: &str = "token-monitor-lite.log";
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

pub(crate) fn timestamp(at: Option<i64>) -> String
{
    at.and_then(DateTime::<Utc>::from_timestamp_millis)
        .map_or_else(|| "none".into(), |time| time.to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[derive(Default)]
pub(crate) struct AppLog
{
    path: Option<PathBuf>,
    writer: Mutex<()>,
}

impl AppLog
{
    pub(crate) fn new(root: &Path) -> Self
    {
        let directory = root.join("logs");
        let path = fs::create_dir_all(&directory).ok().map(|_| directory.join(LOG_FILE));
        let log = Self { path, ..Self::default() };
        log.write(format_args!("startup version={} pid={}", env!("CARGO_PKG_VERSION"), std::process::id()));
        log
    }

    pub(crate) fn write(&self, message: fmt::Arguments<'_>)
    {
        let Some(path) = &self.path else { return };
        // Serialize the size check, rename and append across the request and collection workers.
        let _writer = self.writer.lock().expect("application log");
        let now = Utc::now();
        if fs::metadata(path).is_ok_and(|file| file.len() > MAX_LOG_BYTES)
        {
            let archive = path.with_file_name(format!("token-monitor-lite-{}.log", now.format("%Y%m%d-%H%M%S-%9fZ")));
            if !archive.exists()
            {
                // Keep appending to the current file if Windows cannot rename it yet.
                let _ = fs::rename(path, archive);
            }
        }
        if let Ok(mut file) = fs::OpenOptions::new().append(true).create(true).open(path)
        {
            let stamp = now.to_rfc3339_opts(SecondsFormat::Millis, true);
            let _ = file.write_all(format!("{stamp} {message}\r\n").as_bytes());
        }
    }
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn appends_on_restart_and_rotates_an_oversized_log()
    {
        let root = std::env::temp_dir().join(format!("token-monitor-lite-app-log-{}", std::process::id()));
        let directory = root.join("logs");
        let path = directory.join(LOG_FILE);
        AppLog::new(&root).write(format_args!("first run"));
        let first = fs::read(&path).unwrap();
        let log = AppLog::new(&root);
        let appended = fs::read_to_string(&path).unwrap();
        assert!(appended.as_bytes().starts_with(&first));
        assert_eq!(appended.matches("startup version=").count(), 2);

        // Exercise a rollover during a run and another before the first entry after a restart.
        for message in ["during run", "after restart"]
        {
            let previous = fs::read(&path).unwrap();
            fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(MAX_LOG_BYTES + 1).unwrap();
            if message == "during run"
            {
                log.write(format_args!("{message}"));
            }
            else
            {
                AppLog::new(&root).write(format_args!("{message}"));
            }
            let current = fs::read_to_string(&path).unwrap();
            assert!(current.contains(message));
            assert!(!current.as_bytes().starts_with(&previous));
            let archives: Vec<_> = fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().path())
                .filter(|file| file != &path).collect();
            assert_eq!(archives.len(), 1);
            assert!(archives[0].file_name().unwrap().to_str().unwrap().starts_with("token-monitor-lite-"));
            let archived = fs::read(&archives[0]).unwrap();
            assert_eq!(archived.len() as u64, MAX_LOG_BYTES + 1);
            assert!(archived.starts_with(&previous));
            fs::remove_file(&archives[0]).unwrap();
        }
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
