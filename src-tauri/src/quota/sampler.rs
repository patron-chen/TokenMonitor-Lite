use std::{
    sync::{atomic::Ordering, mpsc::{self, Receiver, RecvTimeoutError, Sender}, Arc, Mutex},
    thread::{self, JoinHandle}, time::{Duration, Instant},
};

use crate::{model::Summary, settings::{Preferences, PreferencesStore, ToolSelection}, sources::Sources};
use super::QuotaCache;

const TOOLS: [&str; 2] = ["codex", "antigravity"];

enum Signal { Usage([u64; 2]), Settings, Stop }

#[derive(Clone)]
pub(crate) struct UsageSignal(Sender<Signal>);

impl UsageSignal
{
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
            run(receiver, &target, &preferences, changed);
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

fn run(receiver: Receiver<Signal>, cache: &QuotaCache, store: &PreferencesStore, changed: impl Fn())
{
    let sources = Sources::discover();
    let mut schedule = Schedule::default();
    loop
    {
        let settings = store.get();
        schedule.update(settings, None);
        let now = Instant::now();
        let interval = Duration::from_secs(u64::from(settings.quotas.refresh_minutes) * 60);
        let tools = schedule.take(now, interval);
        if tools.any()
        {
            cache.sample_local(&sources, tools);
            changed();
        }
        // A pending trailing read has its own deadline, even if no further request is made.
        let now = Instant::now();
        let due = (0..2).filter_map(|i| schedule.due(now, interval, i)).min();
        let signal = match due
        {
            Some(due) => receiver.recv_timeout(due.saturating_duration_since(now)),
            None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match signal
        {
            Ok(Signal::Usage(counts)) => schedule.update(store.get(), Some(counts)),
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
        let last = schedule.take(start + interval, interval);
        assert!(last.codex && !last.antigravity && !last.dsh);
        assert!(!schedule.take(start + interval * 2, interval).any());
        settings.collection.tools.codex = false;
        schedule.update(settings, Some([100, 0]));
        assert!(!schedule.take(start + interval * 2, interval).any());
    }
}
