// Reads capability requests and guest configuration.
use serde_json::{Value, json};
// Configures only the guest's installed helper.
use crate::computer;
// Pins scoped chats to one trusted tool descriptor.
use crate::scoped;

#[derive(Debug, PartialEq)]
pub enum Route {
    Forward(Value),
    Reply(Value),
}

pub struct Policy {
    helper: Option<String>,
}

impl Policy {
    /// Creates the guest's computer policy from its locally resolved helper.
    pub fn new(helper: Option<String>) -> Self {
        Self { helper }
    }

    /// Routes a controller frame without accepting controller executable paths.
    pub fn route(&self, mut frame: Value) -> Route {
        let method = frame["method"].as_str().unwrap_or("");
        if !method.is_empty() && !permitted(method) {
            return reject(&frame, "Guest RPC method is not permitted");
        }
        if matches!(method, "thread/start" | "thread/resume" | "turn/start")
            && !safe_params(method, &frame["params"])
        {
            return reject(&frame, "Guest chat policy overrides are not permitted");
        }
        if method == "miniclaw/computer/read" {
            return Route::Reply(json!({"id": frame["id"],
                "result": {"available": self.helper.is_some()}}));
        }
        if !matches!(method, "thread/start" | "thread/resume") {
            return Route::Forward(frame);
        }
        let config = &frame["params"]["config"];
        if config.as_object().is_some_and(|map| {
            map.iter().any(|(key, value)| {
                key.starts_with("mcp_servers.") || (key == "mcp_servers" && value != &json!({}))
            })
        }) {
            return reject(&frame, "Guest MCP executables are configured locally");
        }
        // Computer Use and the file tool are two ways to act on this VM; a chat carries one.
        // The tools the Mac answers act on nothing here, so they may stay.
        if frame["params"]["miniclawComputerUse"] == true
            && frame["params"]["dynamicTools"].as_array().is_some_and(|tools| tools.iter().any(|tool| tool["name"] == scoped::TOOL)) {
            return reject(&frame, "Scoped and ordinary computer tools cannot be combined");
        }
        let Some(enabled) = frame["params"].get("miniclawComputerUse") else {
            return Route::Forward(frame);
        };
        let Some(enabled) = enabled.as_bool() else {
            return reject(&frame, "Invalid guest computer capability request");
        };
        frame["params"]
            .as_object_mut()
            .unwrap()
            .remove("miniclawComputerUse");
        if enabled {
            let Some(helper) = &self.helper else {
                return reject(&frame, "Computer Use is not installed in the guest");
            };
            computer::configure(&mut frame["params"], helper);
        } else if computer::disable(&mut frame["params"], false).is_err() {
            return reject(&frame, "Guest computer configuration failed");
        }
        Route::Forward(frame)
    }
}

/// Limits the worker channel to the RPC methods used by MiniClaw's local API.
fn permitted(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "initialized"
            | "miniclaw/computer/read"
            | "config/read"
            | "account/read"
            | "account/login/start"
            | "account/login/cancel"
            | "account/rateLimits/read"
            | "model/list"
            | "thread/start"
            | "thread/resume"
            | "thread/list"
            | "thread/read"
            | "thread/turns/list"
            | "thread/items/list"
            | "thread/delete"
            | "thread/archive"
            | "thread/unarchive"
            | "turn/start"
            | "turn/interrupt"
    )
}

/// Accepts current chat fields without permitting alternate tools, roots, or sandboxes.
fn safe_params(method: &str, params: &Value) -> bool {
    let Some(map) = params.as_object() else {
        return false;
    };
    let allowed: &[&str] = if method == "turn/start" {
        &[
            "threadId",
            "clientUserMessageId",
            "environments",
            "additionalContext",
            "input",
            "model",
            "effort",
        ]
    } else {
        &[
            "threadId",
            "cwd",
            "excludeTurns",
            "approvalPolicy",
            "approvalsReviewer",
            "sandbox",
            "runtimeWorkspaceRoots",
            "environments",
            "selectedCapabilityRoots",
            "baseInstructions",
            "developerInstructions",
            "config",
            "model",
            "allowProviderModelFallback",
            "miniclawComputerUse",
            "dynamicTools",
        ]
    };
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return false;
    }
    if let Some(tools) = map.get("dynamicTools") && !owned_tools(tools) {
        return false;
    }
    for key in [
        "environments",
        "selectedCapabilityRoots",
        "runtimeWorkspaceRoots",
    ] {
        if map.get(key).is_some_and(|value| value != &json!([])) {
            return false;
        }
    }
    for (key, expected) in [
        ("sandbox", "read-only"),
        ("approvalPolicy", "never"),
        ("approvalsReviewer", "user"),
    ] {
        if map.get(key).is_some_and(|value| value != expected) {
            return false;
        }
    }
    if let Some(input) = map.get("input") {
        let Some(items) = input.as_array() else {
            return false;
        };
        if items
            .iter()
            .any(|item| item["type"] != "text" || !item["text"].is_string())
        {
            return false;
        }
    }
    let Some(config) = map.get("config") else {
        return true;
    };
    let Some(config) = config.as_object() else {
        return false;
    };
    let defaults: Value =
        serde_json::from_str(include_str!("../chat-policy.json")).expect("bundled chat policy");
    config.iter().all(|(key, value)| {
        if key == "model_reasoning_effort" {
            return value.as_str().is_some_and(|effort| effort.len() <= 32);
        }
        defaults.get(key).is_some_and(|expected| expected == value)
    })
}

/// MiniClaw's tools that are answered by the Mac, not in this VM. `miniclaw/worker/info` names
/// them, and the Mac offers one to a VM conversation only when this worker named it.
pub const MAC_TOOLS: [&str; 3] = [crate::project_tool::TOOL, crate::ask::TOOL, crate::mcp_gateway::TOOL];

/// Accepts the tools of a guest chat: the file and command tool, first and byte for byte, since it
/// is the one that acts in this VM; then any of the tools the Mac answers, each once. Those are
/// checked by name and shape only. Their descriptions are advice that gets reworded, and a guest
/// that compared the words would refuse every conversation until it was updated. They grant the
/// guest nothing: each call goes back to the Mac, which applies its own checks.
fn owned_tools(tools: &Value) -> bool {
    let Some(tools) = tools.as_array() else { return false; };
    let mut seen = Vec::new();
    for (index, tool) in tools.iter().enumerate() {
        if tool == &scoped::tool_definition() {
            if index != 0 { return false; }
            continue;
        }
        let Some(fields) = tool.as_object() else { return false; };
        let Some(name) = tool["name"].as_str() else { return false; };
        if !MAC_TOOLS.contains(&name) || seen.contains(&name) { return false; }
        if fields.keys().any(|key| !["type", "name", "description", "inputSchema"].contains(&key.as_str()))
            || tool["type"] != "function" || !tool["description"].is_string() || !tool["inputSchema"].is_object()
            || tool.to_string().len() > 8 * 1024 { return false; }
        seen.push(name);
    }
    true
}

/// Returns a bounded protocol error without reflecting submitted paths or config.
fn reject(frame: &Value, message: &str) -> Route {
    Route::Reply(json!({"id": frame["id"], "error": {"code": -32602, "message": message}}))
}
