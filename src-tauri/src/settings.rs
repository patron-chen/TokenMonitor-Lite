use std::{fs, path::{Path, PathBuf}, sync::Mutex, time::Duration};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ToolSelection
{
    pub codex: bool,
    pub antigravity: bool,
    pub dsh: bool,
}

impl Default for ToolSelection
{
    fn default() -> Self { Self { codex: true, antigravity: true, dsh: true } }
}

impl ToolSelection
{
    pub const IDS: [&'static str; 3] = ["codex", "antigravity", "dsh"];

    pub fn contains(self, id: &str) -> bool
    {
        match id
        {
            "codex" => self.codex,
            "antigravity" => self.antigravity,
            "dsh" => self.dsh,
            _ => false,
        }
    }

    pub fn enabled(self) -> impl Iterator<Item = &'static str>
    {
        Self::IDS.into_iter().filter(move |id| self.contains(id))
    }

    pub fn any(self) -> bool { self.codex || self.antigravity || self.dsh }

    pub fn scope(self, key: String) -> String
    {
        // Existing all-tool snapshots keep their original range key.
        if self == Self::default() { key }
        else { format!("{key}|tools:{}", self.enabled().collect::<Vec<_>>().join(",")) }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Cadence
{
    #[default]
    #[serde(rename = "live")]
    Live,
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "15m")]
    FifteenMinutes,
    #[serde(rename = "30m")]
    ThirtyMinutes,
}

impl Cadence
{
    pub fn interval(self) -> Option<Duration>
    {
        let minutes = match self
        {
            Self::Live => return None,
            Self::FiveMinutes => 5,
            Self::FifteenMinutes => 15,
            Self::ThirtyMinutes => 30,
        };
        Some(Duration::from_secs(minutes * 60))
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct CollectionSettings
{
    pub cadence: Cadence,
    pub tools: ToolSelection,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QuotaSettings
{
    pub tools: ToolSelection,
    pub refresh_minutes: u8,
}

impl Default for QuotaSettings
{
    fn default() -> Self { Self { tools: ToolSelection::default(), refresh_minutes: 2 } }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StatisticsRange
{
    Week,
    Last7,
    #[default]
    Last30,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences
{
    pub statistics_range: StatisticsRange,
    pub collection: CollectionSettings,
    pub quotas: QuotaSettings,
}

pub struct PreferencesStore
{
    file: PathBuf,
    value: Mutex<Preferences>,
}

impl PreferencesStore
{
    pub fn load(root: &Path) -> Self
    {
        let file = root.join("settings.json");
        let mut value: Preferences = fs::read(&file).ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        if ![1, 2, 5, 15, 30].contains(&value.quotas.refresh_minutes)
        {
            value.quotas.refresh_minutes = QuotaSettings::default().refresh_minutes;
        }
        Self { file, value: Mutex::new(value) }
    }

    pub fn get(&self) -> Preferences
    {
        *self.value.lock().expect("runtime settings")
    }

    pub fn save(&self, next: Preferences) -> Result<Preferences, String>
    {
        if ![1, 2, 5, 15, 30].contains(&next.quotas.refresh_minutes)
        {
            return Err("Invalid quota refresh frequency.".into());
        }
        let mut value = self.value.lock().expect("runtime settings");
        let previous = *value;
        if previous != next
        {
            let encoded = serde_json::to_vec(&next).map_err(|error| error.to_string())?;
            let temporary = self.file.with_extension("tmp");
            fs::write(&temporary, encoded).and_then(|_| fs::rename(&temporary, &self.file))
                .map_err(|_| "Could not save the local settings.".to_string())?;
            *value = next;
        }
        Ok(previous)
    }
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn existing_settings_default_to_live_collection_and_all_tools()
    {
        let settings: Preferences = serde_json::from_str(r#"{"quotas":{"tools":{"codex":false}}}"#).unwrap();
        assert_eq!(settings.statistics_range, StatisticsRange::Last30);
        assert_eq!(settings.collection, CollectionSettings::default());
        assert_eq!(settings.quotas.refresh_minutes, 2);
        assert_eq!(settings.quotas.tools.enabled().collect::<Vec<_>>(), ["antigravity", "dsh"]);
        assert_eq!(settings.collection.tools.scope("today".into()), "today");
        assert_eq!(settings.quotas.tools.scope("today".into()), "today|tools:antigravity,dsh");
    }

    #[test]
    fn statistics_range_matches_the_period_keys_in_saved_settings()
    {
        for (range, key) in [
            (StatisticsRange::Week, "week"), (StatisticsRange::Last7, "last7"), (StatisticsRange::Last30, "last30"),
        ]
        {
            let settings = Preferences { statistics_range: range, ..Preferences::default() };
            let serialized = serde_json::to_value(settings).unwrap();
            assert_eq!(serialized["statisticsRange"], key);
            assert_eq!(serde_json::from_value::<Preferences>(serialized).unwrap(), settings);
        }
    }
}
