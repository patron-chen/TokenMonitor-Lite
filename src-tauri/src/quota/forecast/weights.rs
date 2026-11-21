use std::collections::HashMap;

use super::{Amount, MIN_POINTS};

pub(super) const RECENT_REQUESTS: usize = 20;

#[derive(Clone, Copy)]
pub(super) struct Request
{
    pub at: i64,
    pub amount: Amount,
    pub model: Option<usize>,
    pub speed_factor: f64,
}

impl Request
{
    pub(super) fn billable(&self) -> bool { self.speed_factor > 0.0 && self.amount.tokens > 0 }
}

// Included allowance at the same model's Standard speed, per the 2026-11-21 speed table.
// https://learn.chatgpt.com/docs/pricing#speed-multipliers
fn included_speed(model: &str, tier: &str) -> Option<f64>
{
    match (tier, model)
    {
        ("standard" | "default", _) => Some(1.0),
        ("fast" | "priority", "gpt-6.2-astra" | "gpt-6.2-sol") => Some(2.0),
        ("fast" | "priority", "gpt-6.1-sol" | "gpt-6.1-terra" | "gpt-6-astra" | "gpt-6-sol" | "gpt-6-luna") => Some(2.5),
        ("ultrafast", "gpt-6.2-astra" | "gpt-6.2-sol") => Some(6.0),
        ("ultrafast", "gpt-6-astra" | "gpt-6.1-sol") => Some(8.0),
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct Models(HashMap<(String, Option<String>), usize>);

impl Models
{
    pub(super) fn identify(&mut self, tool: &str, model: &str, tier: Option<&str>) -> (Option<usize>, f64)
    {
        let model = model.trim().to_ascii_lowercase();
        if tool == "codex" && model == "codex-auto-review" { return (None, 0.0); }
        if matches!(model.as_str(), "" | "unknown" | "auto") { return (None, 1.0); }
        let tier = tier.unwrap_or("").trim().to_ascii_lowercase();
        // A missing/undocumented Codex tier learns its own observed rate. Never assume it was Standard.
        let factor = if tool == "codex" { included_speed(&model, &tier) } else { Some(1.0) };
        let id = self.0.len();
        let model = *self.0.entry((model, factor.is_none().then_some(tier))).or_insert(id);
        (Some(model), factor.unwrap_or(1.0))
    }
}

#[derive(Default)]
pub(super) struct Workload
{
    pub amount: Amount,
    pub calls: usize,
    pub oldest: Option<i64>,
    units: HashMap<usize, f64>,
    unknown: bool,
}

impl Workload
{
    pub(super) fn from_requests<'a>(events: impl Iterator<Item = &'a Request>) -> Self
    {
        let mut result = Self::default();
        for event in events.filter(|event| event.billable())
        {
            result.amount.add(event.amount);
            result.calls += 1;
            result.oldest = Some(result.oldest.map_or(event.at, |at| at.min(event.at)));
            if let Some(model) = event.model
            {
                *result.units.entry(model).or_default() += event.amount.tokens as f64 * event.speed_factor;
            }
            else { result.unknown = true; }
        }
        result
    }
}

#[derive(Clone, Copy, Default)]
struct Rate
{
    points: f64,
    units: f64,
    intervals: usize,
}

impl Rate
{
    fn supported(self) -> bool { self.intervals >= 2 && self.points + 1e-9 >= MIN_POINTS && self.units > 0.0 }
}

#[derive(Default)]
pub(super) struct Calibration
{
    pub amount: Amount,
    pub points: f64,
    rates: HashMap<usize, Rate>,
}

impl Calibration
{
    pub(super) fn observe(&mut self, work: &Workload, drop: f64)
    {
        if work.amount.tokens == 0 { return; }
        self.amount.add(work.amount);
        self.points += drop;
        // Shared pools expose one percentage. A mixed interval cannot identify which model caused the drop.
        if work.unknown || work.units.len() != 1 { return; }
        for (&model, &units) in &work.units
        {
            let rate = self.rates.entry(model).or_default();
            rate.units += units;
            rate.points += drop;
            rate.intervals += 1;
        }
    }

    pub(super) fn predict_points(&self, history: &Self, recent: &Workload) -> Option<f64>
    {
        if recent.unknown { return None; }
        let mut points = 0.0;
        for (model, units) in &recent.units
        {
            let mut rate = self.rates.get(model).copied().unwrap_or_default();
            if !rate.supported()
            {
                let old = history.rates.get(model)?;
                rate.units += old.units;
                rate.points += old.points;
                rate.intervals += old.intervals;
                if !rate.supported() { return None; }
            }
            points += units * rate.points / rate.units;
        }
        (points > 0.0).then_some(points)
    }
}

#[derive(Default)]
pub(super) struct Validation
{
    intervals: usize,
    model_error: f64,
    average_error: f64,
}

impl Validation
{
    // Score both methods before adding this interval to their training data.
    pub(super) fn compare(&mut self, work: &Workload, actual: f64, current: &Calibration, history: &Calibration)
    {
        let Some(predicted) = current.predict_points(history, work) else { return };
        let average = if current.points + 1e-9 >= MIN_POINTS { current } else { history };
        if average.points + 1e-9 < MIN_POINTS || average.amount.tokens == 0 { return; }
        let pooled = work.amount.tokens as f64 * average.points / average.amount.tokens as f64;
        self.model_error += (predicted - actual).abs();
        self.average_error += (pooled - actual).abs();
        self.intervals += 1;
    }

    pub(super) fn accepted(&self) -> bool
    {
        self.intervals >= 3 && self.model_error <= self.average_error + 1e-9
    }
}
