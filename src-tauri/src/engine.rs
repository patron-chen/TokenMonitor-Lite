use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use chrono::{Days, NaiveDate, Utc, Weekday};
use notify::{event::{CreateKind, ModifyKind, RemoveKind, RenameMode}, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    app_log::AppLog,
    cache::{Cache, Report}, model::{next_midnight, system_zone, Activity, DailyUsage, Period, SessionPage, Span, Summary, PAGE_SIZE},
    pricing::Pricing,
    quota::sampler::UsageSignal,
    settings::{CollectionSettings, PreferencesStore, ToolSelection},
    sources::Sources,
    session_metadata,
};

mod schedule;
use schedule::Schedule;
pub(crate) use schedule::Interaction;

const QUIET: Duration = Duration::from_secs(3);
const MAX_WAIT: Duration = Duration::from_secs(5);
const SCAN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Default)]
struct Progress
{
    running: bool,
    failure: Option<String>,
}

enum Signal
{
    Scan(Period),
    Activity(bool, mpsc::Sender<Result<Activity, String>>),
    Wake,
    Interaction(Interaction),
    Resume,
    Quit,
}

enum Job
{
    Scan(Period),
    Activity(bool, mpsc::Sender<Result<Activity, String>>),
}

#[derive(Clone, Copy)]
struct Writes
{
    first: Instant,
    last: Instant,
}

impl Writes
{
    fn due(self) -> Instant
    {
        (self.last + QUIET).min(self.first + MAX_WAIT)
    }
}

pub struct Engine
{
    pub sources: Sources,
    pub cache: Cache,
    preferences: Arc<PreferencesStore>,
    signals: mpsc::Sender<Signal>,
    tray: Arc<Mutex<Option<tauri::Rect>>>,
    progress: Arc<Mutex<HashMap<Period, Progress>>>,
    watching: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Engine
{
    pub fn start(
        root: &Path, scanner: PathBuf, preferences: Arc<PreferencesStore>, samples: UsageSignal, log: Arc<AppLog>,
        changed: impl Fn(Period, u64, Option<Summary>) + Send + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>>
    {
        fs::create_dir_all(root)?;
        let config = root.join("tokscale");
        Pricing::seed(&config)?;
        let cache = Cache::new(root.join("usage-v1.sqlite"))?;
        let sources = Sources::discover();
        let (signals, receiver) = mpsc::channel();
        let progress = Arc::new(Mutex::new(HashMap::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let watching = Arc::new(AtomicBool::new(false));
        let tray = Arc::new(Mutex::new(None));
        let worker = Worker {
            cache: cache.clone(), sources: sources.clone(), config, scanner,
            preferences: preferences.clone(), watching: watching.clone(),
            signals: signals.clone(), receiver, progress: progress.clone(), stopped: stopped.clone(), changed,
            tray: tray.clone(), samples, log,
        };
        let worker = thread::Builder::new().name("usage-scan".into()).spawn(move || worker.run())?;
        let engine = Self { cache, sources, preferences, signals, tray, progress, watching, stopped, worker: Mutex::new(Some(worker)) };
        engine.refresh(Period::Today);
        Ok(engine)
    }

    pub fn refresh(&self, period: Period)
    {
        let mut progress = self.progress.lock().expect("scan progress");
        let state = progress.entry(period).or_default();
        if !state.running
        {
            state.running = true;
            state.failure = None;
            let _ = self.signals.send(Signal::Scan(period));
        }
    }

    pub(crate) fn interact(&self, event: Interaction)
    {
        let _ = self.signals.send(Signal::Interaction(event));
    }

    pub(crate) fn tray_hover(&self, bounds: Option<tauri::Rect>)
    {
        let mut previous = self.tray.lock().expect("tray bounds");
        let inside = bounds.is_some();
        if inside != previous.is_some() { self.interact(Interaction::Tray(inside)); }
        *previous = bounds;
    }

    pub fn summary(&self, period: Period) -> rusqlite::Result<Summary>
    {
        let key = self.scope_key(&current_span(period));
        let mut summary = self.cache.summary(period, &key)?;
        let progress = self.progress.lock().expect("scan progress");
        if let Some(state) = progress.get(&period)
        {
            summary.refreshing = state.running;
            summary.failure.clone_from(&state.failure);
        }
        Ok(summary)
    }

    pub fn sessions(&self, period: Period, tool: Option<&str>, query: &str, page: u32, preview: bool) -> rusqlite::Result<SessionPage>
    {
        let limit = if preview { 5 } else { PAGE_SIZE };
        let mut page = self.cache.sessions(period, &self.scope_key(&current_span(period)), tool, query, page, limit)?;
        session_metadata::enrich(&self.sources, &mut page.rows);
        Ok(page)
    }

    pub fn scope_key(&self, span: &Span) -> String
    {
        self.tracked_tools().scope(span.key())
    }

    pub fn tracked_tools(&self) -> ToolSelection { self.preferences.get().collection.tools }

    pub fn settings_changed(&self, period: Period, collection_tools_changed: bool)
    {
        if collection_tools_changed
        {
            // Requeue even if the old set is already being scanned.
            let _ = self.signals.send(Signal::Scan(Period::Today));
            if period != Period::Today { let _ = self.signals.send(Signal::Scan(period)); }
        }
        else
        {
            let _ = self.signals.send(Signal::Wake);
        }
    }

    pub fn collection_status(&self) -> rusqlite::Result<CollectionStatus>
    {
        let data = self.summary(Period::Today)?;
        let sources = ToolSelection::IDS.into_iter().map(|tool| CollectionSource {
            tool,
            roots: self.sources.roots_for(tool).into_iter().map(|root| SourceDirectory {
                exists: root.is_dir(), path: root.to_string_lossy().into_owned(),
            }).collect(),
            tokens: data.updated_at.map(|_| data.tools.iter().find(|row| row.key == tool).map_or(0, |row| row.usage.tokens)),
        }).collect();
        Ok(CollectionStatus {
            timezone: system_zone().to_string(), updated_at: data.updated_at, scanning: data.refreshing,
            failure: data.failure, watching: self.watching.load(Ordering::Relaxed), sources,
        })
    }

    pub fn cached_activity(&self) -> rusqlite::Result<Option<Activity>>
    {
        let span = current_span(Period::Today);
        let tools = self.tracked_tools();
        let today = span.start.expect("today").to_string();
        let days = if tools.any() { self.cache.activity(&tools.scope(span.key()), &today)? }
            else { Some(vec![]) };
        Ok(days.map(|days| Activity { today, days }))
    }

    pub fn activity(&self, force: bool) -> Result<Activity, String>
    {
        if !force
        {
            if let Ok(Some(activity)) = self.cached_activity() { return Ok(activity); }
        }
        let (reply, result) = mpsc::channel();
        self.signals.send(Signal::Activity(force, reply)).map_err(|_| "Usage worker is unavailable.".to_string())?;
        result.recv().map_err(|_| "Usage worker is unavailable.".to_string())?
    }

    pub fn shutdown(&self)
    {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = self.signals.send(Signal::Quit);
        if let Some(worker) = self.worker.lock().expect("scan worker").take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionStatus
{
    timezone: String,
    updated_at: Option<i64>,
    scanning: bool,
    failure: Option<String>,
    watching: bool,
    sources: Vec<CollectionSource>,
}

#[derive(Serialize)]
struct CollectionSource
{
    tool: &'static str,
    roots: Vec<SourceDirectory>,
    tokens: Option<u64>,
}

#[derive(Serialize)]
struct SourceDirectory
{
    path: String,
    exists: bool,
}

pub fn current_span(period: Period) -> Span
{
    Span::new(period, Utc::now(), system_zone(), first_weekday())
}

#[cfg(windows)]
fn first_weekday() -> Weekday
{
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};

    let mut value = [0_u16; 2];
    // Windows stores the regional first day as 0 for Monday through 6 for Sunday.
    unsafe { GetLocaleInfoEx(std::ptr::null(), LOCALE_IFIRSTDAYOFWEEK, value.as_mut_ptr(), 2) };
    match value[0] as u8
    {
        b'1' => Weekday::Tue,
        b'2' => Weekday::Wed,
        b'3' => Weekday::Thu,
        b'4' => Weekday::Fri,
        b'5' => Weekday::Sat,
        b'6' => Weekday::Sun,
        _ => Weekday::Mon,
    }
}

#[cfg(not(windows))]
fn first_weekday() -> Weekday { Weekday::Mon }

struct Worker<F>
{
    log: Arc<AppLog>,
    cache: Cache,
    sources: Sources,
    config: PathBuf,
    scanner: PathBuf,
    preferences: Arc<PreferencesStore>,
    watching: Arc<AtomicBool>,
    signals: mpsc::Sender<Signal>,
    tray: Arc<Mutex<Option<tauri::Rect>>>,
    samples: UsageSignal,
    receiver: mpsc::Receiver<Signal>,
    progress: Arc<Mutex<HashMap<Period, Progress>>>,
    stopped: Arc<AtomicBool>,
    changed: F,
}

impl<F: Fn(Period, u64, Option<Summary>)> Worker<F>
{
    fn run(self)
    {
        let writes = Arc::new(Mutex::new(None));
        let mut settings = self.preferences.get().collection;
        let mut roots = self.watch_roots(settings);
        let mut watcher = open_watcher(roots.clone(), writes.clone(), self.signals.clone());
        let _resume = resume::register(self.signals.clone());
        let mut watches = HashMap::new();
        let mut sync_needed = true;
        let mut work = Work::new();
        let mut period_key = current_span(Period::Today).key();
        let mut revision = 0;
        self.publish(Period::Today, revision);
        while !self.stopped.load(Ordering::Relaxed)
        {
            while let Ok(signal) = self.receiver.try_recv()
            {
                if !work.receive(signal, &self.samples) { return; }
            }
            let preferences = self.preferences.get();
            let requested = preferences.collection;
            if settings != requested
            {
                settings = requested;
                let next_roots = self.watch_roots(settings);
                if roots != next_roots
                {
                    drop(watcher.take());
                    writes.lock().expect("watch timing").take();
                    watches.clear();
                    roots = next_roots;
                    watcher = open_watcher(roots.clone(), writes.clone(), self.signals.clone());
                    sync_needed = true;
                }
                work.schedule.restart(Instant::now());
                revision += 1;
                self.publish(Period::Today, revision);
            }
            let today = current_span(Period::Today);
            let new_key = today.key();
            if period_key != new_key
            {
                period_key = new_key;
                work.invalidate();
            }
            let now = Instant::now();
            let pending = *writes.lock().expect("watch timing");
            let watch_due = pending.filter(|_| work.schedule.active()).map(Writes::due);
            if watch_due.is_some_and(|when| when <= now) { work.scan(Period::Today); }
            let needs_scan = pending.is_some() || !self.watching.load(Ordering::Relaxed);
            if work.check_tray(now, settings, needs_scan) && preferences.quotas.tools.any()
            {
                self.samples.tray_idle();
            }
            if sync_needed || matches!(work.jobs.front(), Some(Job::Scan(Period::Today)))
            {
                let covered = watcher.as_mut().is_some_and(|watcher| sync_roots(watcher, &roots, &mut watches));
                self.watching.store(covered, Ordering::Relaxed);
                sync_needed = false;
            }
            if let Some(job) = work.jobs.pop_front()
            {
                let period = match job
                {
                    Job::Scan(period) => period,
                    Job::Activity(force, reply) =>
                    {
                        let _ = reply.send(self.scan_activity(force, settings.tools));
                        continue;
                    }
                };
                if period == Period::Today
                {
                    // Later notifications belong to the next scan, including changes during this one.
                    writes.lock().expect("watch timing").take();
                    work.invalidated = false;
                }
                {
                    let mut progress = self.progress.lock().expect("scan progress");
                    *progress.entry(period).or_default() = Progress { running: true, failure: None };
                }
                revision += 1;
                self.publish(period, revision);
                let result = self.scan(period, settings.tools);
                if let Err(error) = &result
                {
                    self.log.write(format_args!("usage scan period={period:?} result=failed error={error}"));
                    if period == Period::Today { work.invalidated = true; }
                }
                {
                    let mut progress = self.progress.lock().expect("scan progress");
                    *progress.entry(period).or_default() = Progress { running: false, failure: result.err() };
                }
                revision += 1;
                self.publish(period, revision);
                continue;
            }
            let now = Utc::now();
            let until_midnight = (next_midnight(now, today.zone) - now).to_std().unwrap_or(Duration::ZERO);
            let idle_due = work.schedule.idle_deadline().filter(|_| settings.tools.any() || preferences.quotas.tools.any());
            let until_signal = watch_due.into_iter().chain(idle_due).min()
                .map(|due| due.saturating_duration_since(Instant::now()))
                .unwrap_or(until_midnight).min(until_midnight);
            if let Ok(signal) = self.wait(until_signal, work.schedule.hovered)
            {
                if !work.receive(signal, &self.samples) { return; }
            }
        }
    }

    fn watch_roots(&self, settings: CollectionSettings) -> Vec<PathBuf>
    {
        settings.tools.enabled().flat_map(|tool| self.sources.roots_for(tool)).collect()
    }

    fn wait(&self, delay: Duration, _hovered: bool) -> Result<Signal, mpsc::RecvTimeoutError>
    {
        #[cfg(windows)]
        if _hovered
        {
            let until = Instant::now() + delay;
            loop
            {
                let remaining = until.saturating_duration_since(Instant::now());
                let result = self.receiver.recv_timeout(remaining.min(Duration::from_secs(1)));
                if !matches!(result, Err(mpsc::RecvTimeoutError::Timeout)) || Instant::now() >= until { return result; }
                // Explorer can omit Leave when the pointer jumps out after resting on the icon.
                let mut bounds = self.tray.lock().expect("tray bounds");
                if bounds.as_ref().is_some_and(crate::popover::cursor_outside_tray)
                {
                    *bounds = None;
                    return Ok(Signal::Interaction(Interaction::Tray(false)));
                }
            }
        }
        self.receiver.recv_timeout(delay)
    }

    fn publish(&self, period: Period, revision: u64)
    {
        let today = (period == Period::Today).then(|| {
            let key = self.preferences.get().collection.tools.scope(current_span(period).key());
            let mut summary = self.cache.summary(period, &key).unwrap_or_default();
            if let Some(progress) = self.progress.lock().expect("scan progress").get(&period)
            {
                summary.refreshing = progress.running;
                summary.failure.clone_from(&progress.failure);
            }
            self.samples.updated(&summary);
            summary
        });
        (self.changed)(period, revision, today);
    }

    fn scan(&self, period: Period, tools: ToolSelection) -> Result<(), String>
    {
        let span = current_span(period);
        let mut report = if tools.any()
        {
            read_report(&self.scanner, &self.config, &self.sources, period, &span, tools, &self.stopped)?
        }
        else { Report { entries: vec![], sessions: vec![] } };
        self.sources.fill_codex_titles(&mut report.sessions);
        self.sources.fill_antigravity_titles(&mut report.sessions);
        let prices = Pricing::load();
        self.cache.replace(period, &tools.scope(span.key()), Utc::now().timestamp_millis(), report, |entry| prices.cost(entry))
            .map_err(|_| "Could not update the local usage cache.".into())
    }

    fn scan_activity(&self, force: bool, tools: ToolSelection) -> Result<Activity, String>
    {
        let span = current_span(Period::Today);
        let key = tools.scope(span.key());
        let today = span.start.expect("today");
        let date = today.to_string();
        let mut cached = self.cache.activity(&key, &date).ok().flatten();
        let days = if !tools.any()
        {
            vec![]
        }
        else if let Some(days) = (!force).then(|| cached.take()).flatten()
        {
            days
        }
        else
        {
            let start = today.checked_sub_days(Days::new(230)).expect("history start");
            let end = today.pred_opt().expect("yesterday");
            let mut command = scanner_command(&self.scanner, &self.config, &self.sources, &span, tools, true);
            command.args(["--since", &start.to_string(), "--until", &end.to_string()]);
            let scanned = run_scan::<Graph>(command, &self.stopped).map(|graph| {
                graph.contributions.into_iter().filter_map(|day| {
                    let date = NaiveDate::parse_from_str(&day.date, "%Y-%m-%d").ok()?;
                    (date >= start && date <= end).then_some(DailyUsage { date: day.date, tokens: day.totals.tokens })
                }).collect::<Vec<_>>()
            });
            match scanned
            {
                Ok(days) =>
                {
                    self.cache.replace_activity(&key, Utc::now().timestamp_millis(), &days)
                        .map_err(|_| "Could not update activity history.".to_string())?;
                    days
                }
                Err(error) => cached.ok_or(error)?,
            }
        };
        Ok(Activity { today: date, days })
    }
}

#[derive(Deserialize)]
struct Graph
{
    contributions: Vec<GraphDay>,
}

#[derive(Deserialize)]
struct GraphDay
{
    date: String,
    totals: GraphTotals,
}

#[derive(Deserialize)]
struct GraphTotals
{
    tokens: u64,
}

struct Work
{
    jobs: VecDeque<Job>,
    schedule: Schedule,
    invalidated: bool,
}

impl Work
{
    fn new() -> Self
    {
        Self { jobs: VecDeque::new(), schedule: Schedule::new(Instant::now()), invalidated: false }
    }

    fn receive(&mut self, signal: Signal, samples: &UsageSignal) -> bool
    {
        match signal
        {
            Signal::Quit => return false,
            Signal::Wake => (),
            Signal::Resume => self.invalidate(),
            Signal::Scan(period) => self.scan(period),
            Signal::Activity(force, reply) => self.jobs.push_back(Job::Activity(force, reply)),
            Signal::Interaction(event) =>
            {
                let active = self.schedule.active();
                if self.schedule.interact(event, Instant::now()) { self.scan(Period::Today); }
                if active != self.schedule.active() { samples.collection_active(self.schedule.active()); }
            }
        }
        true
    }

    fn invalidate(&mut self)
    {
        self.invalidated = true;
        if self.schedule.active() { self.scan(Period::Today); }
    }

    fn check_tray(&mut self, now: Instant, settings: CollectionSettings, needs_scan: bool) -> bool
    {
        if !self.schedule.take_idle_check(now, settings.cadence) { return false; }
        // Every idle check can refresh quotas. Only dirty or unmonitored usage needs a scan.
        if settings.tools.any() && (needs_scan || self.invalidated) { self.scan(Period::Today); }
        true
    }

    fn scan(&mut self, period: Period)
    {
        if self.jobs.iter().any(|job| matches!(job, Job::Scan(previous) if *previous == period)) { return; }
        if period == Period::Today { self.jobs.push_front(Job::Scan(period)); }
        else { self.jobs.push_back(Job::Scan(period)); }
    }
}

fn open_watcher(
    roots: Vec<PathBuf>, writes: Arc<Mutex<Option<Writes>>>, sender: mpsc::Sender<Signal>,
) -> Option<RecommendedWatcher>
{
    if roots.is_empty() { return None; }
    notify::recommended_watcher(move |event: notify::Result<Event>| {
        if let Ok(event) = event
        {
            if !event.need_rescan() && (matches!(event.kind, EventKind::Access(_)) ||
                !event.paths.iter().any(|path| relevant(path, &roots, event.kind)))
            {
                return;
            }
        }
        let now = Instant::now();
        let mut writes = writes.lock().expect("watch timing");
        if let Some(pending) = writes.as_mut()
        {
            pending.last = now;
        }
        else
        {
            *writes = Some(Writes { first: now, last: now });
            let _ = sender.send(Signal::Wake);
        }
    }).ok()
}

fn relevant(path: &Path, roots: &[PathBuf], kind: EventKind) -> bool
{
    if roots.iter().any(|root| root.starts_with(path))
    {
        return matches!(kind, EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) | EventKind::Any);
    }
    if !roots.iter().any(|root| path.starts_with(root))
    {
        return false;
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if name.ends_with(".jsonl") || name.ends_with(".jsonl.zst") || name.ends_with(".jsonl.zstd")
        || name.ends_with(".db") || name.ends_with(".db-wal") { return true; }
    if name.ends_with(".db-shm") || name.ends_with(".db-journal") { return false; }
    // Windows also reports Modify(Any) on date directories during a scan. Only directory
    // creation, removal or renaming can bring an entire set of sessions into or out of scope.
    match kind
    {
        EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder | RemoveKind::Any) |
            EventKind::Modify(ModifyKind::Name(RenameMode::From | RenameMode::Both)) => true,
        EventKind::Create(CreateKind::Any) | EventKind::Modify(ModifyKind::Name(_)) | EventKind::Any => path.is_dir(),
        _ => false,
    }
}

fn sync_roots(watcher: &mut RecommendedWatcher, roots: &[PathBuf], watched: &mut HashMap<PathBuf, RecursiveMode>) -> bool
{
    let mut desired = HashMap::new();
    for root in roots
    {
        if root.is_dir()
        {
            desired.insert(root.clone(), RecursiveMode::Recursive);
        }
        if let Some(parent) = root.ancestors().skip(1).find(|path| path.is_dir())
        {
            desired.entry(parent.to_owned()).or_insert(RecursiveMode::NonRecursive);
        }
    }
    watched.retain(|path, mode| {
        if desired.get(path) == Some(mode) { return true; }
        let _ = watcher.unwatch(path);
        false
    });
    let count = desired.len();
    for (path, mode) in desired
    {
        if !watched.contains_key(&path) && watcher.watch(&path, mode).is_ok()
        {
            watched.insert(path, mode);
        }
    }
    count > 0 && watched.len() == count
}

fn read_report(
    scanner: &Path, config: &Path, sources: &Sources, period: Period, span: &Span, tools: ToolSelection, stopped: &AtomicBool,
) -> Result<Report, String>
{
    let mut command = scanner_command(scanner, config, sources, span, tools, false);
    match period
    {
        Period::Today | Period::Week | Period::Last7 | Period::Last30 =>
        {
            // Pass the captured days so a queued scan cannot cross midnight under its readers.
            command.args(["--since", &span.start.expect("dated start").to_string(),
                          "--until", &span.end.expect("dated end").to_string()]);
        }
        Period::Month =>
        {
            use chrono::Months;
            let start = span.start.expect("month start");
            let end = start.checked_add_months(Months::new(1)).and_then(|day| day.pred_opt()).expect("month end");
            command.args(["--since", &start.to_string(), "--until", &end.to_string()]);
        }
        Period::AllTime => {}
    }
    run_scan(command, stopped)
}

fn scanner_command(scanner: &Path, config: &Path, sources: &Sources, span: &Span, tools: ToolSelection, graph: bool) -> Command
{
    let mut command = Command::new(scanner);
    if graph { command.arg("graph"); }
    else { command.args(["--json", "--group-by", "client,workspace,session,model"]); }
    let clients = tools.enabled().map(|tool| if tool == "antigravity" { "antigravity-cli,antigravity-extension" } else { tool })
        .collect::<Vec<_>>().join(",");
    command.args(["--offline", "--bucket-timezone", &span.zone.to_string(), "--no-spinner", "--client", &clients])
        .env("TOKSCALE_CONFIG_DIR", config).env("TOKSCALE_RECOVERY_DISABLE", "1")
        .env_remove("TOKSCALE_EXTRA_DIRS").env_remove("TOKSCALE_HEADLESS_DIR")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(home) = &sources.home_override
    {
        command.arg("--home").arg(home);
    }
    else if tools.antigravity
    {
        let extras = sources.antigravity_extra_dirs();
        if !extras.is_empty() { command.env("TOKSCALE_EXTRA_DIRS", extras); }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
        command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
    }
    command
}

fn run_scan<T: DeserializeOwned + Send>(mut command: Command, stopped: &AtomicBool) -> Result<T, String>
{
    let mut child = command.spawn().map_err(|_| "Could not start the bundled Tokscale scanner.".to_string())?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    thread::scope(|scope| {
        let report = scope.spawn(move || serde_json::from_reader(BufReader::new(stdout)));
        let warnings = scope.spawn(move || {
            let mut text = Vec::new();
            let mut pipe = BufReader::new(stderr);
            let mut head = pipe.by_ref().take(65536);
            let _ = head.read_to_end(&mut text);
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            String::from_utf8_lossy(&text).to_ascii_lowercase()
        });
        let started = Instant::now();
        let result = loop
        {
            if stopped.load(Ordering::Relaxed) || started.elapsed() > SCAN_TIMEOUT
            {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Tokscale scan timed out or was cancelled.".to_string());
            }
            match child.try_wait()
            {
                Ok(Some(status)) if status.success() => break Ok(()),
                Ok(Some(_)) => break Err("Tokscale could not finish the scan.".into()),
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) =>
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err("Could not retrieve the Tokscale scan.".into());
                }
            }
        };
        result?;
        let warnings = warnings.join().expect("scanner diagnostics");
        if warnings.contains("database is locked") || warnings.contains("database is busy")
        {
            return Err("A source database is busy. Refresh to retry.".into());
        }
        report.join().expect("scanner report parser")
            .map_err(|_| "Tokscale returned an invalid report.".into())
    })
}

#[derive(Clone, Serialize)]
pub struct Update
{
    pub period: Period,
    pub revision: u64,
}

#[cfg(windows)]
mod resume
{
    use std::{ffi::c_void, ptr};
    use windows_sys::Win32::{
        Foundation::HANDLE,
        System::Power::{PowerRegisterSuspendResumeNotification, PowerUnregisterSuspendResumeNotification,
            DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS},
        UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC},
    };
    use super::{mpsc, Signal};

    pub struct Subscription
    {
        handle: HANDLE,
        _sender: Box<mpsc::Sender<Signal>>,
    }

    unsafe extern "system" fn awake(context: *const c_void, kind: u32, _: *const c_void) -> u32
    {
        if kind == PBT_APMRESUMEAUTOMATIC
        {
            let sender = unsafe { &*(context.cast::<mpsc::Sender<Signal>>()) };
            let _ = sender.send(Signal::Resume);
        }
        0
    }

    pub fn register(sender: mpsc::Sender<Signal>) -> Option<Subscription>
    {
        let mut sender = Box::new(sender);
        let mut params = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(awake),
            Context: (&mut *sender as *mut mpsc::Sender<Signal>).cast(),
        };
        let mut handle = ptr::null_mut();
        let result = unsafe { PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, (&mut params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast(), &mut handle) };
        (result == 0).then_some(Subscription { handle, _sender: sender })
    }

    impl Drop for Subscription
    {
        fn drop(&mut self)
        {
            if !self.handle.is_null()
            {
                unsafe { PowerUnregisterSuspendResumeNotification(self.handle as isize) };
            }
        }
    }
}

#[cfg(not(windows))]
mod resume
{
    pub fn register(_: std::sync::mpsc::Sender<super::Signal>) {}
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn cached_activity_reads_past_days_without_the_usage_worker()
    {
        let root = std::env::temp_dir().join(format!("lite-activity-cache-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("usage-v1.sqlite");
        let (signals, receiver) = mpsc::channel();
        // An activity lookup that queues a scan will fail immediately in this fixture.
        drop(receiver);
        let engine = Engine {
            sources: Sources::from_home(Some(root.clone())),
            cache: Cache::new(file.clone()).unwrap(),
            preferences: Arc::new(PreferencesStore::load(&root)),
            signals,
            tray: Arc::default(), progress: Arc::default(), watching: Arc::default(), stopped: Arc::default(),
            worker: Mutex::new(None),
        };
        let span = current_span(Period::Today);
        let today = span.start.unwrap();
        let yesterday = today.pred_opt().unwrap().to_string();
        assert!(engine.cached_activity().unwrap().is_none());
        engine.cache.replace_activity(&engine.scope_key(&span), Utc::now().timestamp_millis(), &[
            DailyUsage { date: yesterday.clone(), tokens: 42 },
            DailyUsage { date: today.to_string(), tokens: 999 },
        ]).unwrap();

        let activity = engine.activity(false).unwrap();
        assert_eq!(activity.today, today.to_string());
        assert_eq!(activity.days.len(), 1);
        assert_eq!((activity.days[0].date.as_str(), activity.days[0].tokens), (yesterday.as_str(), 42));
        assert!(engine.activity(true).is_err());
        drop(engine);
        for name in ["usage-v1.sqlite-wal", "usage-v1.sqlite-shm"]
        {
            let sidecar = root.join(name);
            if sidecar.exists() { fs::remove_file(sidecar).unwrap(); }
        }
        fs::remove_file(file).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn continuous_log_notifications_do_not_activate_the_tray_or_restart_backoff()
    {
        let start = Instant::now();
        let mut work = Work::new();
        work.schedule.restart(start);
        let samples = UsageSignal::disconnected();
        let settings = CollectionSettings::default();
        let mut checks = Vec::new();

        // Even a wakeup on every write must leave today's scan on the next idle slot.
        for second in 1..=60 * 60
        {
            let now = start + Duration::from_secs(second);
            assert!(work.receive(Signal::Wake, &samples));
            assert!(!work.schedule.active() && work.jobs.is_empty());
            if work.check_tray(now, settings, true)
            {
                checks.push(second);
                assert!(matches!(work.jobs.pop_front(), Some(Job::Scan(Period::Today))));
            }
            assert!(work.jobs.is_empty());
        }
        assert_eq!(checks, [60, 240, 540, 1140, 2340, 3540]);
    }

    #[test]
    fn idle_checks_continue_with_clean_logs_or_with_only_quotas_enabled()
    {
        let mut settings = CollectionSettings::default();
        let start = Instant::now();
        let mut work = Work::new();
        work.schedule.restart(start);
        assert!(work.check_tray(start + Duration::from_secs(60), settings, false));
        assert!(work.jobs.is_empty());
        settings.tools = ToolSelection { codex: false, antigravity: false, dsh: false };
        // Quota-only mode has no watcher and still reaches the next idle check.
        assert!(!work.check_tray(start + Duration::from_secs(239), settings, true));
        assert!(work.check_tray(start + Duration::from_secs(240), settings, true));
        assert!(work.jobs.is_empty());
    }

    #[test]
    fn coalescing_is_bounded_and_filters_generated_sqlite_writes()
    {
        let start = Instant::now();
        assert_eq!(Writes { first: start, last: start }.due(), start + QUIET);
        assert_eq!(Writes { first: start, last: start + Duration::from_secs(4) }.due(), start + MAX_WAIT);
        let roots = vec![PathBuf::from("antigravity/conversations")];
        let modified = EventKind::Modify(ModifyKind::Any);
        assert!(relevant(Path::new("antigravity/conversations/a.db-wal"), &roots, modified));
        assert!(!relevant(Path::new("antigravity/conversations/a.db-shm"), &roots, modified));
        assert!(!relevant(Path::new("tokscale/cache/a.jsonl"), &roots, modified));
        assert!(relevant(&roots[0], &roots, EventKind::Create(CreateKind::Any)));
    }

    #[test]
    fn directory_metadata_notifications_do_not_schedule_another_scan()
    {
        let roots = [PathBuf::from(env!("CARGO_MANIFEST_DIR"))];
        let child = roots[0].join("src");
        let modified = EventKind::Modify(ModifyKind::Any);
        assert!(!relevant(&roots[0], &roots, modified));
        assert!(!relevant(&child, &roots, modified));
        assert!(relevant(&child, &roots, EventKind::Create(CreateKind::Any)));
        assert!(relevant(&child, &roots, EventKind::Modify(ModifyKind::Name(RenameMode::From))));
    }
}
