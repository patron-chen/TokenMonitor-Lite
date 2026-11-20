use std::{fmt, fs, io::Write, path::{Path, PathBuf}};

use chrono::{SecondsFormat, Utc};

#[derive(Default)]
pub(super) struct BankedResetLog
{
    pub(super) path: Option<PathBuf>,
}

impl BankedResetLog
{
    pub(super) fn new(root: &Path) -> Self
    {
        let directory = root.join("logs");
        let path = fs::create_dir_all(&directory).ok().map(|_| directory.join("codex-banked-reset.log"));
        let log = Self { path };
        log.begin(format_args!("startup version={} diagnostics=2", env!("CARGO_PKG_VERSION")));
        log
    }

    pub(super) fn begin(&self, message: fmt::Arguments<'_>)
    {
        // Keep a single shareable file. Start a fresh one between reads, never midway through a request.
        if let Some(path) = &self.path
        {
            if fs::metadata(path).is_ok_and(|file| file.len() >= 128 * 1024)
            {
                let _ = fs::File::create(path);
            }
        }
        self.write(message);
    }

    pub(super) fn write(&self, message: fmt::Arguments<'_>)
    {
        let Some(path) = &self.path else { return };
        if let Ok(mut file) = fs::OpenOptions::new().append(true).create(true).open(path)
        {
            let stamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            let _ = file.write_all(format!("{stamp} {message}\r\n").as_bytes());
        }
    }
}
