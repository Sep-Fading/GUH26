// Defines the minimal health route and response.
use axum::{Json, Router, routing::get};
// Builds the backend health response payload.
use serde_json::{Value, json};

pub mod logging;
pub mod platform_fs;
pub mod powershell;
pub mod windows_network_broker;
pub mod windows_winsock_acl;
pub mod windows_git;
pub mod windows_shell;
#[cfg(windows)]
mod windows_conpty;
#[cfg(windows)]
pub mod windows_process;
pub mod provider_process;
pub mod rpc;
pub mod worker;
#[cfg(unix)]
pub mod channel;
pub mod remote;
pub mod api;
pub mod chat;
pub mod chat_folder;
pub mod runtime;
pub mod claude;
pub mod claude_login;
pub mod providers;
pub mod tool_bridge;
pub mod frames;
pub mod models;
pub mod computer;
pub mod guest_computer;
pub mod scoped;
#[cfg_attr(windows, path = "executor_windows.rs")]
pub mod executor;
pub mod shell_access;
pub mod seatbelt;
#[cfg(target_os = "linux")]
pub mod sandbox_linux;
pub mod egress;
pub mod history;
pub mod search;
pub mod transcripts;
pub mod appearance;
pub mod idle;
pub mod transfer;
pub mod transfer_jobs;
pub mod backups;
pub mod activity;
pub mod delegations;
pub mod schedules;
pub mod queue;
pub mod ask;
pub mod attachments;
pub mod briefing;
pub mod secrets;
pub mod mcp_servers;
pub mod mcp_import;
pub mod mcp_client;
pub mod mcp_gateway;
#[cfg(windows)]
pub mod mcp_npm;
pub mod recovery;
pub mod agents;
pub mod bundles;
pub mod main_agent;
pub mod capability_defaults;
pub mod names;
pub mod speeds;
pub mod analytics;
pub mod project_history;
pub mod issue_tool;
pub mod issues;
pub mod project_memory;
pub mod project_tool;
pub mod terminal;
pub mod harmless;
pub mod skills;
pub mod slash;
pub mod skill_groups;
pub mod projects;
pub mod environment;
pub mod permissions;

/// Returns the minimal health-test router without starting Codex or serving the UI.
pub fn app() -> Router {
    Router::new().route("/api/health", get(health))
}

/// Returns a backend-only health response for the minimal router.
async fn health() -> Json<Value> {
    Json(json!({ "backend": "ok", "platform": std::env::consts::OS }))
}
pub mod notifications;

pub mod windows_sandbox;
pub mod windows_gate;
pub mod windows_runtimes;
pub mod npm_launch;

#[cfg(windows)]
pub mod windows_network;

#[cfg(windows)]
mod windows_security_lock;
pub mod windows_ports;

pub mod workspace_commands;
