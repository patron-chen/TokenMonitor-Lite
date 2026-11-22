use std::{
    sync::{atomic::Ordering, mpsc::{self, Receiver, RecvTimeoutError, Sender}, Arc, Mutex},
    thread::{self, JoinHandle}, time::{Duration, Instant},
};

use crate::{model::Summary, settings::{Preferences, PreferencesStore, ToolSelection}, sources::Sources};
use super::{QuotaCache, QuotaRefresh};

const TOOLS: [&str; 2] = ["codex", "antigravity"];

enum Signal { Usage([u64; 2]), CollectionActive(bool), TrayIdle, Settings, Stop }

#[derive(Clone)]
pub(crate) struct UsageSignal(Sender<Signal>);

impl UsageSignal
{
    #[cfg(test)]
    pub(crate) fn disconnected() -> Self
    {
        Self(mpsc::channel().0)
    }

    pub fn collection_active(&self, active: bool)
    {
        let _ = self.0.send(Signal::CollectionActive(active));
    }

    pub fn tray_idle(&self)
    {
        let _ = self.0.send(Signal::TrayIdle);
    }

    pub fn updated(&self, summary: &Summary)
    {
        if summary.refreshing || summary.failure.is_some() || summary.updated_at.is_none() { return; }
        let counts = TOOLS.map(|tool| summary.tools.iter().find(|row| row.key == tool).map_or(0, |row| row.usage.tokens));
        let _ = self.0.send(Signal::Usage(counts));
    }
}

pub(crate) struct Sampler
{
    signal: Sender<Signal>,
    cache: Arc<QuotaCache>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct Schedule
{
    enabled: [bool; 2],
    pending: [bool; 2],
    last: [Option<Instant>; 2],
    counts: Option<[u64; 2]>,
}

impl Schedule
{
    fn update(&mut self, settings: Preferences, counts: Option<[u64; 2]>)
    {
        for (i, tool) in TOOLS.iter().enumerate()
        {
            let enabled = settings.collection.tools.contains(tool) && settings.quotas.tools.contains(tool);
            let changed = counts.is_some_and(|new| self.counts.map_or(new[i] > 0, |old| old[i] != new[i]));
            self.pending[i] = enabled && (self.pending[i] || !self.enabled[i] || changed);
            if !enabled { self.last[i] = None; }
            self.enabled[i] = enabled;
        }
        if counts.is_some() { self.counts = counts; }
    }

    fn due(&self, now: Instant, interval: Duration, i: usize) -> Option<Instant>
    {
        self.pending[i].then(|| self.last[i].map_or(now, |last| last + interval))
    }

    fn deadline(&self, active: bool, now: Instant, interval: Duration) -> Option<Instant>
    {
        // In the tray, pending observations share the usage worker's next idle check.
        if !active { return None; }
        (0..2).filter_map(|i| self.due(now, interval, i)).min()
    }

    fn take(&mut self, now: Instant, interval: Duration) -> ToolSelection
    {
        let [codex, antigravity] = std::array::from_fn(|i| {
            if !self.due(now, interval, i).is_some_and(|due| due <= now) { return false; }
            self.pending[i] = false;
            self.last[i] = Some(now);
            true
        });
        ToolSelection { codex, antigravity, dsh: false }
    }
}

impl Sampler
{
    pub fn start(
        cache: Arc<QuotaCache>, preferences: Arc<PreferencesStore>, changed: impl Fn() + Send + 'static,
    ) -> std::io::Result<(Self, UsageSignal)>
    {
        let (signal, receiver) = mpsc::channel();
        let target = cache.clone();
        let worker = thread::Builder::new().name("quota-sampler".into()).spawn(move || {
            run(receiver, &target, &preferences, Sources::discover(), changed);
        })?;
        Ok((Self { signal: signal.clone(), cache, thread: Mutex::new(Some(worker)) }, UsageSignal(signal)))
    }

    pub fn settings_changed(&self) { let _ = self.signal.send(Signal::Settings); }

    pub fn shutdown(&self)
    {
        self.cache.stopping.store(true, Ordering::Relaxed);
        let _ = self.signal.send(Signal::Stop);
        if let Some(worker) = self.thread.lock().expect("quota sampler").take() { let _ = worker.join(); }
    }
}

fn run(receiver: Receiver<Signal>, cache: &QuotaCache, store: &PreferencesStore, sources: Sources, changed: impl Fn())
{
    let mut schedule = Schedule::default();
    let mut active = false;
    let mut startup = true;
    while !cache.stopping.load(Ordering::Relaxed)
    {
        let settings = store.get();
        schedule.update(settings, None);
        let now = Instant::now();
        let interval = Duration::from_secs(u64::from(settings.quotas.refresh_minutes) * 60);
        let tools = schedule.take(now, interval);
        if startup
        {
            let codex_online = settings.quotas.tools.codex;
            let result = if tools.any() || codex_online { "scheduled" } else { "no_enabled_tools" };
            cache.log.write(format_args!(
                "quota startup result={result} codex_collection={} codex_quota={} antigravity_collection={} antigravity_quota={} refresh_minutes={} codex_online={codex_online} banked_reset=skipped_at_startup",
                settings.collection.tools.codex, settings.quotas.tools.codex,
                settings.collection.tools.antigravity, settings.quotas.tools.antigravity, settings.quotas.refresh_minutes));
            cache.sample_startup(&sources, tools, codex_online.then(|| || cache.fetch_startup_codex(&sources)), &changed);
            startup = false;
        }
        else if tools.any()
        {
            cache.sample_local(&sources, tools, "background", active);
            changed();
        }
        let now = Instant::now();
        let signal = match schedule.deadline(active, now, interval)
        {
            Some(due) => receiver.recv_timeout(due.saturating_duration_since(now)),
            None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match signal
        {
            Ok(Signal::Usage(counts)) => schedule.update(store.get(), Some(counts)),
            Ok(Signal::CollectionActive(value)) => active = value,
            Ok(Signal::TrayIdle) =>
            {
                let quotas = store.get().quotas;
                if quotas.tools.any()
                {
                    cache.read(&sources, quotas, QuotaRefresh::TrayIdle);
                    changed();
                }
            }
            Ok(Signal::Settings) | Err(RecvTimeoutError::Timeout) => (),
            Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn each_idle_signal_refreshes_quota_only_settings_inside_the_panel_interval()
    {
        let root = std::env::temp_dir().join(format!("token-monitor-tray-sampler-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = PreferencesStore::load(&root);
        let mut settings = Preferences::default();
        settings.collection.tools = ToolSelection { codex: false, antigravity: false, dsh: false };
        settings.quotas.tools = ToolSelection { dsh: true, ..settings.collection.tools };
        settings.quotas.refresh_minutes = 30;
        store.save(settings).unwrap();
        let cache = QuotaCache::default();
        let (sender, receiver) = mpsc::channel();
        let usage = UsageSignal(sender.clone());
        usage.tray_idle();
        usage.tray_idle();
        sender.send(Signal::Stop).unwrap();

        // No usage notification or panel request. The fixture isolates all provider calls.
        let published = std::cell::Cell::new(0);
        run(receiver, &cache, &store, Sources::from_home(Some(root.clone())), || published.set(published.get() + 1));
        assert_eq!(published.get(), 2);
        assert!(cache.cached(settings.quotas.tools).unwrap().checked_at.is_some());
        std::fs::remove_file(root.join("settings.json")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn takes_a_baseline_then_coalesces_the_last_request_even_without_more_signals()
    {
        let mut schedule = Schedule::default();
        let mut settings = Preferences::default();
        let start = Instant::now();
        let interval = Duration::from_secs(120);
        schedule.update(settings, None);
        assert!(schedule.take(start, interval).antigravity);
        schedule.update(settings, Some([25, 0]));
        schedule.update(settings, Some([80, 0]));
        assert!(!schedule.take(start + Duration::from_secs(5), interval).any());
        assert_eq!(schedule.due(start, interval, 0), Some(start + interval));
        assert_eq!(schedule.deadline(false, start, interval), None);
        assert_eq!(schedule.deadline(true, start, interval), Some(start + interval));
        let last = schedule.take(start + interval, interval);
        assert!(last.codex && !last.antigravity && !last.dsh);
        assert!(!schedule.take(start + interval * 2, interval).any());
        settings.collection.tools.codex = false;
        schedule.update(settings, Some([100, 0]));
        assert!(!schedule.take(start + interval * 2, interval).any());
    }
}
