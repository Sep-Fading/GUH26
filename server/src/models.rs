// Tracks catalog duplicates and sets read deadlines.
use std::{collections::HashSet, time::Duration};

// Converts model fields between Rust and JSON.
use serde::{Deserialize, Serialize};
// Builds paginated requests for Codex models.
use serde_json::json;

// Requests model pages and reports transport failures.
use crate::rpc::{Client, Failure};

#[derive(Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A speed tier's id, from the model's `service_tiers`; none means standard.
    pub speed: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub hidden: bool,
    pub is_default: bool,
    pub default_reasoning_effort: String,
    pub supported_reasoning_efforts: Vec<Effort>,
    // Absent for Codex models; "claude" routes a new chat to the Claude bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The speed tiers the model offers (Codex 0.155 lists them per model); a tier's id is what
    /// a turn sends as `serviceTier`. Empty for a model with one speed, and for Claude.
    #[serde(default)]
    pub service_tiers: Vec<Tier>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Tier {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effort {
    // Codex supplies the effort names; keep new values usable without an app update.
    pub reasoning_effort: String,
    pub description: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    data: Vec<Model>,
    next_cursor: Option<String>,
}

/// Returns the full model catalog, limiting the combined page reads to ten seconds.
pub async fn read(client: &Client) -> Result<Vec<Model>, Failure> {
    tokio::time::timeout(Duration::from_secs(10), pages(client, false))
        .await
        .map_err(|_| Failure::Timeout)?
}

/// Bypasses the Claude bridge's discovery cache. Codex uses its ordinary model/list.
pub async fn refresh_claude(client: &Client) -> Result<Vec<Model>, Failure> {
    tokio::time::timeout(Duration::from_secs(10), pages(client, true))
        .await
        .map_err(|_| Failure::Timeout)?
}

/// Collects and deduplicates model pages, including hidden entries; rejects repeated
/// cursors, malformed replies, or a catalog beyond the configured limits.
async fn pages(client: &Client, refresh: bool) -> Result<Vec<Model>, Failure> {
    let mut models = Vec::new();
    let mut cursor = None;
    let mut cursors = HashSet::new();
    let mut names = HashSet::new();
    for _ in 0..100 {
        let mut params = json!({ "limit": 100, "includeHidden": true, "cursor": cursor });
        if refresh && cursor.is_none() { params["refresh"] = json!(true); }
        let result = client
            .call("model/list", params)
            .await?;
        let page: Page =
            serde_json::from_value(result).map_err(|_| Failure::Unavailable)?;
        for model in page.data {
            if model.model.is_empty() {
                return Err(Failure::Unavailable);
            }
            if names.insert(model.model.clone()) {
                models.push(model);
            }
        }
        if models.len() > 10000 {
            return Err(Failure::Unavailable);
        }
        let Some(next) = page.next_cursor else {
            return Ok(models);
        };
        // A repeated cursor would keep fetching the same pages.
        if !cursors.insert(next.clone()) {
            return Err(Failure::Unavailable);
        }
        cursor = Some(next);
    }
    Err(Failure::Unavailable)
}
