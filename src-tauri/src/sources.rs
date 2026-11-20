use std::{collections::{HashMap, HashSet}, env, fs, io::{BufRead, BufReader}, path::{Path, PathBuf}};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use crate::cache::SessionMeta;

#[derive(Clone, Debug)]
pub struct Sources
{
    pub home_override: Option<PathBuf>,
    codex: PathBuf,
    dsh: PathBuf,
    antigravity_cli: PathBuf,
    antigravity_ide: PathBuf,
    antigravity_alternates: [PathBuf; 2],
    cc_switch_db: PathBuf,
}

impl Sources
{
    pub fn discover() -> Self
    {
        let home_override = env_path("TOKEN_MONITOR_LITE_SOURCE_HOME").filter(|path| path.is_absolute());
        Self::from_home(home_override)
    }

    pub(crate) fn from_home(home_override: Option<PathBuf>) -> Self
    {
        let home = home_override.clone().or_else(|| env_path("HOME").filter(|path| path.is_absolute()))
            .or_else(|| env_path("USERPROFILE")).unwrap_or_else(env::temp_dir);
        let root = |name: &str, fallback: PathBuf| {
            if home_override.is_some() { fallback } else { env_path(name).and_then(|path| std::path::absolute(path).ok()).unwrap_or(fallback) }
        };
        Self {
            codex: root("CODEX_HOME", home.join(".codex")),
            dsh: root("DSH_HOME", home.join(".dsh")).join("sessions"),
            antigravity_cli: root("GEMINI_CLI_HOME", home.join(".gemini")).join("antigravity-cli/conversations"),
            antigravity_ide: home.join(".gemini/antigravity/conversations"),
            antigravity_alternates: [
                home.join(".gemini/antigravity-ide/conversations"),
                home.join(".gemini/antigravity-backup/conversations"),
            ],
            cc_switch_db: home.join(".cc-switch/cc-switch.db"),
            home_override,
        }
    }

    pub fn roots_for(&self, tool: &str) -> Vec<PathBuf>
    {
        match tool
        {
            "codex" => vec![self.codex_sessions(), self.codex.join("archived_sessions")],
            "antigravity" => std::iter::once(&self.antigravity_cli).chain(self.antigravity_ide_roots()).cloned().collect(),
            "dsh" => vec![self.dsh.clone()],
            _ => vec![],
        }
    }

    pub fn codex_sessions(&self) -> PathBuf
    {
        self.codex.join("sessions")
    }

    pub fn codex_root(&self) -> &Path
    {
        &self.codex
    }

    pub fn codex_auth(&self) -> PathBuf
    {
        self.codex.join("auth.json")
    }

    pub fn cc_switch_db(&self) -> &Path
    {
        &self.cc_switch_db
    }

    pub fn codex_file(&self, id: &str) -> Option<PathBuf>
    {
        if !safe_id(id)
        {
            return None;
        }
        let roots = [self.codex.join("sessions"), self.codex.join("archived_sessions")];
        let dated = id.strip_prefix("rollout-").and_then(|rest| rest.get(..10)).filter(|date| {
            date.bytes().enumerate().all(|(index, byte)| if index == 4 || index == 7 { byte == b'-' } else { byte.is_ascii_digit() })
        });
        for extension in ["jsonl", "jsonl.zst"]
        {
            let name = format!("{id}.{extension}");
            for root in &roots
            {
                let flat = root.join(&name);
                if flat.is_file()
                {
                    return Some(flat);
                }
                if let Some(date) = dated
                {
                    let candidate = root.join(date.replace('-', "/")).join(&name);
                    if candidate.is_file()
                    {
                        return Some(candidate);
                    }
                }
            }
            for root in &roots
            {
                if let Some(file) = find_file(root, &name, 12)
                {
                    return Some(file);
                }
            }
        }
        None
    }

    pub fn dsh_files(&self, id: &str) -> Vec<PathBuf>
    {
        if !safe_id(id)
        {
            return Vec::new();
        }
        let mut fallback = Vec::new();
        for project in directory_paths(&self.dsh)
        {
            for session in directory_paths(&project)
            {
                let mut files: Vec<_> = fs::read_dir(&session).into_iter().flatten().filter_map(Result::ok)
                    .filter_map(|entry| {
                        let name = entry.file_name();
                        let (version, plain) = dsh_version(name.to_str()?)?;
                        entry.path().is_file().then_some((version, plain, entry.path()))
                    }).collect();
                files.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
                if let Some((_, _, path)) = files.into_iter().next()
                {
                    // Prefer the session directory, then try the header IDs on demand.
                    if session.file_name().is_some_and(|name| name == id)
                    {
                        return vec![path];
                    }
                    fallback.push(path);
                }
            }
        }
        fallback
    }

    pub fn antigravity_files(&self, id: &str, sources: &[String]) -> Vec<PathBuf>
    {
        if !safe_id(id)
        {
            return Vec::new();
        }
        let file = format!("{id}.db");
        sources.iter().filter_map(|source| match source.as_str()
        {
            "antigravity-cli" => Some(self.antigravity_cli.join(&file)),
            "antigravity-extension" => self.antigravity_ide_roots().map(|root| root.join(&file)).find(|path| path.is_file()),
            _ => None,
        }).filter(|path| path.is_file()).collect()
    }

    pub fn antigravity_status(&self) -> (bool, bool)
    {
        let mut protobuf = false;
        for path in self.antigravity_ide_roots().chain(std::iter::once(&self.antigravity_cli))
        {
            for entry in fs::read_dir(path).into_iter().flatten().filter_map(Result::ok)
            {
                match entry.path().extension().and_then(|ext| ext.to_str())
                {
                    Some("db") => return (true, false),
                    Some("pb") => protobuf = true,
                    _ => {}
                }
            }
        }
        let pb_only = protobuf || self.antigravity_alternates.iter().any(|path| {
            fs::read_dir(path).into_iter().flatten().filter_map(Result::ok)
                .any(|entry| entry.path().extension().is_some_and(|extension| extension == "pb"))
        });
        (false, pb_only)
    }

    fn antigravity_ide_roots(&self) -> impl Iterator<Item = &PathBuf>
    {
        std::iter::once(&self.antigravity_ide).chain(self.antigravity_alternates.iter()
            .filter(|_| self.home_override.is_none()))
    }

    pub fn antigravity_extra_dirs(&self) -> String
    {
        // Alternate IDE folders can hold a second copy of a migrated conversation.
        // Scan one database per ID, preferring the standard IDE or CLI path.
        if self.home_override.is_some() || !self.antigravity_alternates.iter().any(|root| root.is_dir())
        {
            return String::new();
        }
        let mut seen: HashSet<_> = [&self.antigravity_ide, &self.antigravity_cli]
            .into_iter().flat_map(|root| databases(root)).map(|(name, _)| name).collect();
        let mut locations = Vec::new();
        for root in &self.antigravity_alternates
        {
            let files = databases(root);
            let count = files.len();
            let unique: Vec<_> = files.into_iter().filter_map(|(name, path)| seen.insert(name).then_some(path)).collect();
            if unique.is_empty() { continue; }
            if unique.len() == count { locations.push(root.clone()); }
            else { locations.extend(unique); }
        }
        locations.into_iter().map(|path| format!("antigravity-extension:{}", path.display())).collect::<Vec<_>>().join(",")
    }

    pub fn fill_antigravity_titles(&self, sessions: &mut [SessionMeta])
    {
        if !sessions.iter().any(|row| row.source.starts_with("antigravity-") && row.title.is_none()) { return; }
        for (root, source) in self.antigravity_ide_roots().map(|root| (root, "antigravity-extension"))
            .chain(std::iter::once((&self.antigravity_cli, "antigravity-cli")))
        {
            let Some(parent) = root.parent() else { continue };
            let database = parent.join("conversation_summaries.db");
            if !database.is_file() { continue; }
            let Ok(connection) = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY) else { continue };
            let Ok(mut select) = connection.prepare("SELECT title FROM conversation_summaries WHERE conversation_id = ?1") else { continue };
            for row in sessions.iter_mut().filter(|row| row.source == source && row.title.is_none())
            {
                let id = &row.session_id;
                if let Ok(Some(title)) = select.query_row([id], |match_row| match_row.get::<_, Option<String>>(0))
                {
                    let title: String = title.trim().chars().take(160).collect();
                    if !title.is_empty() { row.title = Some(title); }
                }
            }
        }
    }

    pub fn fill_codex_titles(&self, sessions: &mut [SessionMeta])
    {
        if !sessions.iter().any(|session| session.source == "codex" && session.title.is_none())
        {
            return;
        }
        let Ok(index) = fs::File::open(self.codex.join("session_index.jsonl")) else { return };
        let titles: HashMap<String, String> = BufReader::new(index).lines().map_while(Result::ok)
            .filter_map(|line| serde_json::from_str::<TitleRecord>(&line).ok())
            .filter_map(|record| {
                let title: String = record.thread_name.trim().chars().take(160).collect();
                (!title.is_empty()).then_some((record.id, title))
            }).collect();
        for session in sessions.iter_mut().filter(|session| session.source == "codex" && session.title.is_none())
        {
            if let Some(id) = codex_index_id(&session.session_id)
            {
                session.title = titles.get(id).cloned();
            }
        }
    }
}

#[derive(Deserialize)]
struct TitleRecord
{
    id: String,
    thread_name: String,
}

pub(crate) fn codex_index_id(key: &str) -> Option<&str>
{
    // The canonical rollout spelling has one UUID after its 19-character
    // timestamp. Multi-ID rollouts require an explicit identity binding.
    let id = if key.len() == 36 { key } else { key.strip_prefix("rollout-")?.get(20..)? };
    (id.len() == 36 && id.bytes().enumerate().all(|(at, byte)| {
        if [8, 13, 18, 23].contains(&at) { byte == b'-' } else { byte.is_ascii_hexdigit() }
    })).then_some(id)
}

fn safe_id(id: &str) -> bool
{
    !id.is_empty() && id != "." && id != ".." &&
        id.len() <= 240 && id.bytes().all(|value| value.is_ascii_alphanumeric() || b"._-".contains(&value))
}

fn env_path(name: &str) -> Option<PathBuf>
{
    let path = env::var(name).ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn directory_paths(path: &Path) -> impl Iterator<Item = PathBuf>
{
    fs::read_dir(path).into_iter().flatten().filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir())).map(|entry| entry.path())
}

fn databases(root: &Path) -> Vec<(String, PathBuf)>
{
    fs::read_dir(root).into_iter().flatten().filter_map(Result::ok).filter_map(|entry| {
        let path = entry.path();
        if !path.extension()?.eq_ignore_ascii_case("db") || !path.is_file() { return None; }
        Some((path.file_stem()?.to_str()?.to_ascii_lowercase(), path))
    }).collect()
}

fn find_file(dir: &Path, name: &str, depth: u8) -> Option<PathBuf>
{
    for entry in fs::read_dir(dir).ok()?.filter_map(Result::ok)
    {
        let path = entry.path();
        if entry.file_name() == name && path.is_file()
        {
            return Some(path);
        }
        if depth > 0 && entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            if let Some(file) = find_file(&path, name, depth - 1)
            {
                return Some(file);
            }
        }
    }
    None
}

fn dsh_version(name: &str) -> Option<(u32, bool)>
{
    let plain = !name.ends_with(".zstd");
    let base = name.strip_suffix(".zstd").unwrap_or(name).strip_suffix(".jsonl")?;
    if base == "session"
    {
        return Some((0, plain));
    }
    Some((base.strip_prefix("session.v")?.parse().ok()?, plain))
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn source_names_are_bounded_to_session_ids()
    {
        assert!(safe_id("rollout-2026-11-20T10-00-00-id"));
        assert!(!safe_id("../tokens.jsonl"));
        assert!(!safe_id("d:\\keys"));
        assert_eq!(dsh_version("session.v12.jsonl.zstd"), Some((12, false)));
        assert_eq!(dsh_version("session.jsonl"), Some((0, true)));
        assert_eq!(dsh_version("session.v.jsonl"), None);
        let id = "816c744f-cc7c-4531-a7e1-4f4025ae9f01";
        assert_eq!(codex_index_id(&format!("rollout-2026-11-19T17-56-38-{id}")), Some(id));
        assert_eq!(codex_index_id(&format!("rollout-2026-11-19T17-56-38-{id}-{id}")), None);
    }
}
