//! Ephemeral shell access, owned and timed by the backend.
// Shares conversation bindings and permission checks.
use crate::{chat::Backend, permissions::{Context, Target}};
// Names each grant and encodes results.
use {uuid::Uuid, serde_json::{Value, json}};

#[derive(Clone)]
pub struct Grant {
    pub id: Uuid,
    pub context: Context,
    pub expires_at: i64,
    /// Made by a swarm the user started with unsandboxed shell on (api/swarm.rs): it has no
    /// expiry and does not depend on the agent's Unsandboxed shell switch.
    pub swarm: bool,
}

impl Backend {
    /// Returns the current live grant, never a persisted permission rule.
    pub async fn shell_grant(&self, thread: &str) -> Option<Grant> {
        self.shell_grants.lock().await.get(thread)
            .filter(|grant| grant.expires_at > chrono::Utc::now().timestamp_millis()).cloned()
    }

    /// Executes one unrestricted shell while its original grant and conversation remain valid.
    pub async fn run_unrestricted(&self, thread: &str, turn: &str, grant: Grant,
        text: &str, cwd: &str) -> Value {
        // A swarm grant has no end of its own; one command still stops after a day.
        let remaining = (grant.expires_at - chrono::Utc::now().timestamp_millis()).clamp(0, 86_400_000) as u64;
        let failure = || json!({ "status": "failed", "text":
            "Unsandboxed shell access expired or was revoked. The command was stopped. Ask the user to grant access again in Tools." });
        let watched = async {
            let execution = crate::executor::execute_unrestricted(text, cwd);
            tokio::pin!(execution);
            loop {
                let valid = self.shell_grant(thread).await.is_some_and(|current| current.id == grant.id)
                    && self.agents.permission_context(thread.to_owned()).await.ok().as_ref() == Some(&grant.context)
                    && (grant.swarm || self.agents.configured_switch(grant.context.agent(), grant.context.project(),
                        &Target::Feature { name: "unsandboxed_shell".into() }).await.unwrap_or(false))
                    && self.data.lock().await.chats.get(thread)
                        .is_some_and(|chat| !chat.stopping && chat.active.as_deref() == Some(turn));
                if !valid {
                    return failure();
                }
                tokio::select! {
                    result = &mut execution => return result,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_millis(remaining), watched).await
            .unwrap_or_else(|_| failure())
    }
}
