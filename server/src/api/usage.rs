// Reuses provider authorization and response helpers.
use super::{Error, Json, Query, Result, Shared, State, StatusCode, Value, json};
// Validates provider selectors and refresh flags.
use serde::Deserialize;
// Selects each provider's existing runtime client.
use crate::providers::Id;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Choice {
    provider: Option<String>,
    #[serde(default)]
    refresh: bool,
}

/// Reads one provider's limits; Claude readings are cached for thirty seconds unless
/// explicitly refreshed. A failed read preserves only that provider's previous reading.
pub(super) async fn read(State(backend): State<Shared>, query: std::result::Result<Query<Choice>,
    axum::extract::rejection::QueryRejection>) -> Result<Json<Value>> {
    let Query(choice) = query.map_err(|_| invalid())?;
    let id = Id::parse(choice.provider.as_deref().unwrap_or("codex")).ok_or_else(invalid)?;
    let identity = match id {
        Id::Codex => { super::codex_authorized(&backend).await?; backend.data.lock().await.account.clone() }
        Id::Claude => {
            let account = super::claude_account(&backend).await.ok_or_else(super::claude_unavailable)?;
            if account["status"] != "signed_in" { return Err(super::sign_in_required()); }
            account
        }
    };
    let generation = backend.data.lock().await.generation;
    if id == Id::Claude && !choice.refresh {
        let data = backend.data.lock().await;
        if data.generation != generation || data.claude_account != identity { return Err(super::unavailable()); }
        if let Some(value) = data.usage.get(id.name())
            && value["status"] == "fresh"
            && value["fetchedAt"].as_str().and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
                .is_some_and(|date| (0..30).contains(&(chrono::Utc::now().timestamp() - date.timestamp()))) {
            return Ok(Json(value.clone()));
        }
    }
    let client = backend.provider_client(id).ok_or_else(super::unavailable)?;
    let result = client.call("account/rateLimits/read", json!({})).await.ok();
    let reading = result.as_ref().and_then(|raw| match id {
        Id::Codex => Some(codex(raw)), Id::Claude => claude(raw),
    });
    let mut data = backend.data.lock().await;
    let current = if id == Id::Codex { &data.account } else { &data.claude_account };
    if data.generation != generation || *current != identity { return Err(super::unavailable()); }
    let value = if let Some((buckets, allowed)) = reading {
        json!({ "provider": id.name(), "status": "fresh", "buckets": buckets,
            "ordinaryUsageAllowed": allowed, "fetchedAt": chrono::Utc::now().to_rfc3339() })
    } else {
        let mut value = data.usage.get(id.name()).cloned().unwrap_or(json!({
            "provider": id.name(), "status": "unavailable", "buckets": [],
            "ordinaryUsageAllowed": null, "fetchedAt": null,
        }));
        if value["fetchedAt"].is_string() { value["status"] = json!("stale"); }
        value
    };
    data.usage.insert(id.name().to_owned(), value.clone());
    Ok(Json(value))
}

/// Rejects an unknown provider or malformed refresh query.
fn invalid() -> Error {
    Error(StatusCode::BAD_REQUEST, "invalid_usage_query", "Choose a supported provider.", false)
}

/// Normalizes Codex's named buckets, preserving its explicit send restriction and unknowns.
fn codex(raw: &Value) -> (Vec<Value>, Option<bool>) {
    let bucket = |id: &str, value: &Value| {
        let window = |key: &str| value.get(key).filter(|v| v.is_object()).map(|v| json!({
            "usedPercent": v["usedPercent"].as_f64(), "resetsAt": v["resetsAt"].as_i64(),
            "windowDurationMins": v["windowDurationMins"].as_i64(),
        }));
        json!({ "limitId": id, "limitName": value["limitName"].as_str().unwrap_or(id),
            "primary": window("primary"), "secondary": window("secondary") })
    };
    let buckets = if let Some(map) = raw["rateLimitsByLimitId"].as_object() {
        map.iter().map(|(id, value)| bucket(id, value)).collect()
    } else if raw["rateLimits"].is_object() {
        vec![bucket(raw["rateLimits"]["limitId"].as_str().unwrap_or("codex"), &raw["rateLimits"])]
    } else { Vec::new() };
    (buckets, raw["ordinaryUsageAllowed"].as_bool())
}

/// Converts Claude's percentage and ISO reset time, preserving partial/unknown windows.
fn window(raw: &Value, minutes: Option<i64>) -> Value {
    if !raw.is_object() { return Value::Null; }
    json!({ "usedPercent": raw["utilization"].as_f64().or_else(|| raw["percent"].as_f64()),
        "resetsAt": raw["resets_at"].as_str()
            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok()).map(|date| date.timestamp()),
        "windowDurationMins": minutes })
}

/// Collects Claude's plan, model/surface-specific and enabled spend limits. This
/// experimental response is allowlisted; no account details or raw provider fields escape.
fn claude(raw: &Value) -> Option<(Vec<Value>, Option<bool>)> {
    if raw["rate_limits_available"] != true { return None; }
    let limits = raw.get("rate_limits")?.as_object()?;
    let mut buckets = Vec::new();
    let rows = limits.get("limits").and_then(Value::as_array);
    let main = |kind, legacy| rows.and_then(|rows| rows.iter().find(|row| row["kind"] == kind))
        .or_else(|| limits.get(legacy)).unwrap_or(&Value::Null);
    let primary = window(main("session", "five_hour"), Some(300));
    let secondary = window(main("weekly_all", "seven_day"), Some(10080));
    if !primary.is_null() || !secondary.is_null() {
        buckets.push(json!({ "limitId": "claude", "limitName": "Claude", "primary": primary, "secondary": secondary }));
    }
    if let Some(rows) = rows {
        for (index, row) in rows.iter().enumerate() {
            if matches!(row["kind"].as_str(), Some("session" | "weekly_all")) { continue; }
            let model = row["scope"]["model"]["display_name"].as_str();
            let surface = row["scope"]["surface"]["display_name"].as_str();
            let name = [model, surface].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let name = if name.is_empty() { row["kind"].as_str().unwrap_or("Other limit").replace('_', " ") } else { name };
            let weekly = row["group"] == "weekly";
            buckets.push(json!({ "limitId": format!("claude-{index}"), "limitName": name,
                "primary": if weekly { Value::Null } else { window(row, None) },
                "secondary": if weekly { window(row, Some(10080)) } else { Value::Null } }));
        }
    } else {
        let scoped = limits.get("model_scoped").and_then(Value::as_array);
        if let Some(scoped) = scoped {
            for (index, row) in scoped.iter().enumerate() {
                buckets.push(json!({ "limitId": format!("claude-model-{index}"),
                    "limitName": row["display_name"].as_str().unwrap_or("Model limit"),
                    "primary": null, "secondary": window(row, Some(10080)) }));
            }
        }
        for (key, name) in [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet"),
            ("seven_day_oauth_apps", "OAuth apps"), ("seven_day_cowork", "Cowork")] {
            if scoped.is_some_and(|rows| rows.iter().any(|row| row["display_name"] == name)) { continue; }
            if let Some(raw) = limits.get(key).filter(|raw| raw.is_object()) {
                buckets.push(json!({ "limitId": key, "limitName": name,
                    "primary": null, "secondary": window(raw, Some(10080)) }));
            }
        }
    }
    if let Some(spend) = limits.get("spend").filter(|raw| raw["enabled"] == true) {
        buckets.push(json!({ "limitId": "claude-spend", "limitName": "Usage credits",
            "primary": window(spend, None), "secondary": null }));
    } else if let Some(extra) = limits.get("extra_usage").filter(|raw| raw["is_enabled"] == true) {
        buckets.push(json!({ "limitId": "claude-extra", "limitName": "Extra usage",
            "primary": window(extra, None), "secondary": null }));
    }
    if buckets.is_empty() { return None; }
    Some((buckets, None))
}
