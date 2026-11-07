// Parses machine choices without treating Codex tool environments as workers.
use serde::{Deserialize, Serialize};
// Reads durable bindings through the private metadata store.
use crate::agents::{Issue, Store};
// Handles chats created before a machine was selected.
use rusqlite::OptionalExtension;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ProjectChoice {
    Host {},
    Vm {
        #[serde(default)]
        dedicated: bool,
        #[serde(rename = "vmId", skip_serializing_if = "Option::is_none")]
        vm_id: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ConversationChoice {
    Host {},
    Vm {},
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Assignment {
    Host {},
    Vm {
        #[serde(rename = "vmId")]
        vm_id: String,
    },
}

impl Assignment {
    /// Pins a new conversation to the chosen machine and returns its durable target.
    pub fn master_or_host(choice: ConversationChoice) -> Self {
        match choice {
            ConversationChoice::Host {} => Self::Host {},
            ConversationChoice::Vm {} => Self::Vm { vm_id: "master".into() },
        }
    }

    /// Applies a project's saved VM identity to a new task, or uses Master without one.
    pub fn for_project(choice: ConversationChoice, project: Option<&ProjectChoice>) -> Self {
        match (choice, project) {
            (ConversationChoice::Host {}, _) => Self::Host {},
            (ConversationChoice::Vm {}, Some(ProjectChoice::Vm {
                dedicated: true, vm_id: Some(id),
            })) => Self::Vm { vm_id: id.clone() },
            (ConversationChoice::Vm {}, _) => Self::Vm { vm_id: "master".into() },
        }
    }

    /// Returns the public Host/VM choice without exposing routing as a client override.
    pub fn choice(&self) -> ConversationChoice {
        match self {
            Self::Host {} => ConversationChoice::Host {},
            Self::Vm { .. } => ConversationChoice::Vm {},
        }
    }

    /// Checks that a stored machine identity is one MiniClaw can resolve.
    pub fn valid(&self) -> bool {
        match self {
            Self::Host {} => true,
            Self::Vm { vm_id } => vm_id == "master" || uuid::Uuid::parse_str(vm_id).is_ok(),
        }
    }
}

impl ProjectChoice {
    /// Reports whether saving this choice requires a dedicated VM to be ready.
    pub fn needs_dedicated_vm(&self) -> bool {
        matches!(self, Self::Vm { dedicated: true, .. })
    }

    /// Rejects a forged or mismatched dedicated ID before a project is saved.
    pub fn valid(&self) -> bool {
        match self {
            Self::Host {} => true,
            Self::Vm { dedicated: false, vm_id: None } => true,
            Self::Vm { dedicated: true, vm_id: Some(id) } =>
                uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == *id),
            _ => false,
        }
    }

    /// Returns the pinned dedicated identity, if one has been provisioned.
    pub fn dedicated_id(&self) -> Option<&str> {
        match self {
            Self::Vm { dedicated: true, vm_id: Some(id) } => Some(id),
            _ => None,
        }
    }
}

impl Store {
    /// Lists durable VM task IDs and machine identities independently of workers.
    pub async fn vm_bindings(&self) -> Result<Vec<(String, String)>, Issue> {
        self.run(|db| {
            let mut query = db.prepare("SELECT thread_id, assignment FROM chat_environments")
                .map_err(Issue::storage)?;
            let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?,
                row.get::<_, String>(1)?))).map_err(Issue::storage)?;
            let mut tasks = Vec::new();
            for row in rows {
                let (id, text) = row.map_err(Issue::storage)?;
                let assignment: Assignment = serde_json::from_str(&text).map_err(Issue::storage)?;
                if !assignment.valid() { return Err(Issue::Storage); }
                if let Assignment::Vm { vm_id } = assignment {
                    tasks.push((id, vm_id));
                }
            }
            Ok(tasks)
        }).await
    }

    /// Lists durable Master task IDs while its Codex worker is unavailable.
    pub async fn master_tasks(&self) -> Result<Vec<String>, Issue> {
        Ok(self.vm_bindings().await?.into_iter()
            .filter_map(|(thread, vm)| (vm == "master").then_some(thread)).collect())
    }

    /// Returns a chat's pinned machine, or none when an older chat needs migration.
    pub async fn assignment(&self, thread: String) -> Result<Option<Assignment>, Issue> {
        self.check_creation(thread.clone()).await?;
        self.deletion_assignment(thread).await
    }

    /// Reads a deletion target without lifting a quarantined chat's execution ban.
    pub async fn deletion_assignment(&self, thread: String) -> Result<Option<Assignment>, Issue> {
        self.run(move |db| {
            let text: Option<String> = db.query_row(
                "SELECT assignment FROM chat_environments WHERE thread_id = ?1",
                [&thread], |row| row.get(0),
            ).optional().map_err(Issue::storage)?;
            let Some(text) = text else { return Ok(None); };
            let assignment: Assignment = serde_json::from_str(&text).map_err(Issue::storage)?;
            if !assignment.valid() { return Err(Issue::Storage); }
            Ok(Some(assignment))
        }).await
    }

    /// Gives an older chat one machine assignment after its worker is verified.
    pub async fn assign_existing(&self, thread: String, assignment: Assignment)
        -> Result<Assignment, Issue>
    {
        if uuid::Uuid::parse_str(&thread).is_err() || !assignment.valid() {
            return Err(Issue::Invalid);
        }
        self.check_creation(thread.clone()).await?;
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let old: Option<String> = tx.query_row(
                "SELECT assignment FROM chat_environments WHERE thread_id = ?1",
                [&thread], |row| row.get(0),
            ).optional().map_err(Issue::storage)?;
            if let Some(old) = old {
                let old: Assignment = serde_json::from_str(&old).map_err(Issue::storage)?;
                if old != assignment { return Err(Issue::ConversationLocked); }
                return Ok(old);
            }
            let required: Option<i64> = tx.query_row(
                "SELECT required FROM chat_admissions WHERE thread_id = ?1",
                [&thread], |row| row.get(0),
            ).optional().map_err(Issue::storage)?;
            if required.is_some_and(|bits| bits & 8 != 0) { return Err(Issue::Storage); }
            tx.execute("INSERT INTO chat_environments (thread_id, assignment) VALUES (?1, ?2)",
                rusqlite::params![thread, serde_json::to_string(&assignment).map_err(Issue::storage)?])
                .map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(assignment)
        }).await
    }
}
