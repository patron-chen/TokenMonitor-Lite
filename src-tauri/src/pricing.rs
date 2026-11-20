use std::{collections::HashMap, fs, io, path::Path};

use serde::Deserialize;

use crate::cache::Entry;

const SNAPSHOT: &str = include_str!("../resources/pricing-litellm.json");

#[derive(Deserialize)]
pub struct Pricing
{
    data: HashMap<String, Price>,
}

#[derive(Deserialize)]
struct Price
{
    litellm_provider: String,
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
    cache_creation_input_token_cost: Option<f64>,
    reasoning_cost_per_token: Option<f64>,
}

impl Pricing
{
    pub fn seed(config: &Path) -> io::Result<()>
    {
        let folder = config.join("cache");
        fs::create_dir_all(&folder)?;
        let path = folder.join("pricing-litellm.json");
        if fs::read_to_string(&path).as_deref().ok() != Some(SNAPSHOT)
        {
            // Serialized with scans. The file belongs only to this app's offline profile.
            fs::write(path, SNAPSHOT)?;
        }
        Ok(())
    }

    pub fn load() -> Self
    {
        serde_json::from_str(SNAPSHOT).expect("vendored pricing snapshot")
    }

    pub fn cost(&self, entry: &Entry) -> (Option<f64>, bool)
    {
        let counts = entry.counts;
        if counts.total() == 0
        {
            return (Some(0.0), false);
        }
        let provider = match entry.provider.as_str()
        {
            "google" | "google-ai-studio" => "gemini",
            provider => provider,
        };
        let model = entry.model.as_str();
        let prefixed = format!("{provider}/{model}");
        let price = [prefixed.as_str(), model].into_iter().filter_map(|id| self.data.get(id))
            .find(|price| price.litellm_provider == provider);
        let Some(price) = price else { return (None, true) };
        let uses = [
            (counts.input, price.input_cost_per_token),
            (counts.output, price.output_cost_per_token),
            (counts.cache_read, price.cache_read_input_token_cost),
            (counts.cache_write, price.cache_creation_input_token_cost),
            (counts.reasoning, price.reasoning_cost_per_token.or(price.output_cost_per_token)),
        ];
        let mut missing = false;
        let mut known = None;
        for (count, rate) in uses
        {
            let count = count.unwrap_or(0);
            if count == 0 { continue; }
            if let Some(rate) = rate
            {
                known = Some(known.unwrap_or(0.0) + rate * count as f64);
            }
            else
            {
                missing = true;
            }
        }
        // Preserve Tokscale's request-level tier pricing for complete rows.
        // A zero from its report can also mean it could not resolve this model.
        if !missing && entry.cost.is_finite() && entry.cost > 0.0
        {
            known = Some(entry.cost);
        }
        (known, missing)
    }
}

#[cfg(test)]
mod tests
{
    use super::*;
    use serde_json::json;

    #[test]
    fn pinned_price_and_unknown_remain_distinguishable()
    {
        let table = Pricing::load();
        let mut row: Entry = serde_json::from_value(json!({
            "client":"codex","model":"gpt-5.4","provider":"openai",
            "input":80,"cacheRead":20,"output":6,"reasoning":4,"cost":0.000355
        })).unwrap();
        assert_eq!(table.cost(&row), (Some(row.cost), false));
        row.model = "not-a-priced-fixture".into();
        assert_eq!(table.cost(&row), (None, true));
    }
}
