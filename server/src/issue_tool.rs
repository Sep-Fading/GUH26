//! The issue actions of the project tool. The calling conversation's agent is the actor; the
//! store enforces ownership, handovers and the review level, and every answer says why.

// The backend, its store and the conversation's agent.
use crate::chat::Backend;
// Issues, their actor and the reasons a change is refused.
use crate::issues::{self, Actor, Draft, Record, Refusal};
// Shapes the tool's answer.
use serde_json::{Value, json};
// Names the project.
use uuid::Uuid;

/// An answer for the model: success or not, and words.
fn result(success: bool, text: &str) -> Value {
    json!({ "success": success, "contentItems": [{ "type": "inputText", "text": text }] })
}

/// Names the agent a conversation belongs to; a conversation with none is the main agent's.
async fn actor(backend: &Backend, thread: &str) -> Actor {
    // A swarm agent acts as itself (Dev-1, Review-1), the name its issues carry.
    if let Some(actor) = crate::api::swarm_actor(thread) { return actor; }
    match backend.agents.binding(thread.to_owned(), false).await.ok().flatten() {
        Some(agent) => Actor { id: agent.id.to_string(), name: agent.definition.name.clone() },
        None => Actor { id: "main".into(), name: "Main agent".into() },
    }
}

/// Says why a change was refused, in words a model can act on.
fn why(refusal: Refusal, number: i64) -> String {
    match refusal {
        Refusal::Shape => "That is not a valid request. A title is one line of at most 120 characters, text is 1 byte to 8 KB, \
            kind is bug or feature, severity is low, medium or high, and at most 8 one-word tags.".into(),
        Refusal::Missing => format!("No issue #{number} in this project. Use issue_list to find it."),
        Refusal::Full => "This project already has 500 issues.".into(),
        Refusal::Owned(owner) => format!("#{number} is owned by {owner}. Only its owner works on it and decides when it is done. \
            Add what you know with issue_comment, or ask the user to reassign it."),
        Refusal::NotOwner => format!("You do not own #{number}. Only its owner may finish or release it."),
        Refusal::Unowned => format!("Nobody owns #{number}. Claim it with issue_claim first."),
        Refusal::Closed => format!("#{number} is waiting for the user's review or closed, so agents cannot move it. The user can reopen it."),
        Refusal::Lower => "Agents may raise severity but not lower it. Ask the user if it should be lower.".into(),
        Refusal::Storage => "MiniClaw could not change the issue.".into(),
    }
}

/// One line per issue, for lists and near-duplicates.
fn line(issue: &Record) -> String {
    let status = issues::stage_word(&issue.status);
    format!("#{} [{}] {} · {} · {} · owner: {}{}", issue.number, issue.kind, issue.title, issues::word(&issue.severity), status,
        issue.owner.as_deref().unwrap_or("nobody"),
        if issue.tags.is_empty() { String::new() } else { format!(" · tags: {}", issue.tags.join(", ")) })
}

/// Reads a string argument, trimmed.
fn text(arguments: &Value, key: &str) -> String { arguments[key].as_str().unwrap_or_default().trim().to_owned() }

/// Runs one issue action and answers the Activity status, title and output, and the result.
pub async fn run(backend: &Backend, thread: &str, project: Uuid, action: &str, arguments: &Value) -> (&'static str, String, String, Value) {
    let number = arguments["issue"].as_i64().unwrap_or(0);
    let me = actor(backend, thread).await;
    let store = &backend.agents;
    let done = |title: String, text: String| ("ok", title, text.clone(), result(true, &text));
    let refused = |title: String, refusal: Refusal| { let text = why(refusal, number); ("refused", title, text.clone(), result(false, &text)) };
    // A swarm's Lead may give stories to its agents and change the review level, when the user asks.
    if matches!(action, "issue_assign" | "issue_review_level") {
        let (title, outcome) = crate::api::swarm_lead_action(backend, thread, project, action, arguments).await;
        return match outcome {
            Ok(text) => ("ok", title, text.clone(), result(true, &text)),
            Err(text) => ("refused", title, text.clone(), result(false, &text)),
        };
    }
    match action {
        "issue_list" => {
            let query = Some(text(arguments, "query")).filter(|query| !query.is_empty());
            let status = Some(text(arguments, "status")).filter(|status| !status.is_empty());
            let Ok(found) = store.issues(project, query.clone(), status).await else { return refused("Listed issues".into(), Refusal::Storage) };
            let mut listed = found.iter().take(30).map(line).collect::<Vec<_>>().join("\n");
            if found.len() > 30 { listed.push_str(&format!("\n…and {} more; narrow it with query or status.", found.len() - 30)); }
            if listed.is_empty() { listed = "No issues match.".into(); }
            done(format!("Listed issues{}", query.map(|query| format!(": {query}")).unwrap_or_default()), listed)
        }
        "issue_read" => match store.issue(project, number).await {
            Ok(Some(issue)) => {
                let history = issue.events.iter().flatten().map(|event| format!("- {} {}{}", event.author, event.kind,
                    if event.text.is_empty() { String::new() } else { format!(": {}", event.text) })).collect::<Vec<_>>().join("\n");
                done(format!("Read issue #{number}"), format!("#{} {}\nKind: {} · Severity: {} · Stage: {} · Owner: {} · Tags: {}\n\n{}\n\nHistory:\n{}",
                    issue.number, issue.title, issue.kind, issues::word(&issue.severity), issues::stage_word(&issue.status), issue.owner.as_deref().unwrap_or("nobody"),
                    issue.tags.join(", "), issue.text, history))
            }
            Ok(None) => refused(format!("Read issue #{number}"), Refusal::Missing),
            Err(_) => refused(format!("Read issue #{number}"), Refusal::Storage),
        },
        "issue_create" => {
            let tags = arguments["tags"].as_array().into_iter().flatten().filter_map(|tag| tag.as_str().map(str::to_owned)).collect();
            let draft = Draft { kind: text(arguments, "kind"), title: text(arguments, "title"), text: text(arguments, "text"),
                severity: text(arguments, "severity"), tags };
            let words: Vec<String> = draft.title.to_lowercase().split_whitespace().filter(|word| word.len() > 3).map(str::to_owned).collect();
            match store.create_issue(project, draft, me.name).await {
                Ok(issue) => {
                    let similar: Vec<String> = store.issues(project, None, None).await.unwrap_or_default().iter()
                        .filter(|other| other.number != issue.number && other.status != "closed")
                        .filter(|other| { let title = other.title.to_lowercase(); words.iter().any(|word| title.contains(word.as_str())) })
                        .take(5).map(line).collect();
                    let mut answer = format!("Filed #{}: {} ({}). Claim it with issue_claim before you work on it.", issue.number, issue.title,
                        issues::word(&issue.severity));
                    if !similar.is_empty() {
                        answer.push_str(&format!("\nOpen issues with similar titles; if one is the same, comment there instead:\n{}", similar.join("\n")));
                    }
                    done(format!("Filed #{}: {}", issue.number, issue.title), answer)
                }
                Err(refusal) => refused("Filed an issue".into(), refusal),
            }
        }
        "issue_comment" => match store.comment_issue(project, number, me.name, text(arguments, "text")).await {
            Ok(()) => done(format!("Commented on #{number}"), format!("Added to the history of #{number}.")),
            Err(refusal) => refused(format!("Commented on #{number}"), refusal),
        },
        "issue_claim" => match store.claim_issue(project, number, me.clone()).await {
            Ok(()) => {
                let level = store.review_level(project).await.unwrap_or_else(|_| "low".into());
                done(format!("Claimed #{number}"), format!("You own #{number} now. In this project {}. Hand parts to other agents if you need to; \
                    to stop, issue_release it with a handover.", issues::level_sentence(&level)))
            }
            Err(refusal) => refused(format!("Claimed #{number}"), refusal),
        },
        "issue_release" => match store.release_issue(project, number, me, text(arguments, "text")).await {
            Ok(()) => done(format!("Released #{number}"), format!("Released #{number}. Your handover is in its history for the next owner.")),
            Err(Refusal::Shape) => {
                let text = "A release needs a handover in text: what you found, what is done, and what is left.".to_owned();
                ("refused", format!("Released #{number}"), text.clone(), result(false, &text))
            }
            Err(refusal) => refused(format!("Released #{number}"), refusal),
        },
        "issue_done" => match store.finish_issue(project, number, me, text(arguments, "text")).await {
            Ok(status) if status == "closed" => done(format!("Closed #{number}"),
                format!("Closed #{number}. Its severity closes without review in this project.")),
            Ok(_) => {
                let level = store.review_level(project).await.unwrap_or_else(|_| "low".into());
                done(format!("Marked #{number} done, waiting for review"), format!("#{number} now waits for the user's review: in this project {}. \
                    Tell the user it is ready for them in Issues. You still own it.", issues::level_sentence(&level)))
            }
            Err(Refusal::Shape) => {
                let text = "Say in text what was done, so the user and later agents can check it.".to_owned();
                ("refused", format!("Marked #{number} done"), text.clone(), result(false, &text))
            }
            Err(refusal) => refused(format!("Marked #{number} done"), refusal),
        },
        "issue_stage" => {
            let stage = text(arguments, "stage");
            match store.set_issue_stage(project, number, me, stage.clone()).await {
                Ok(()) => done(format!("Moved #{number} to {}", issues::stage_word(&stage)), format!("#{number} is in {} now.", issues::stage_word(&stage))),
                Err(Refusal::Shape) => {
                    let text = "stage is dev, qa or agent_review. Human Review and Closed come from issue_done and the user.".to_owned();
                    ("refused", format!("Moved #{number}"), text.clone(), result(false, &text))
                }
                Err(refusal) => refused(format!("Moved #{number}"), refusal),
            }
        }
        "issue_severity" => {
            let severity = text(arguments, "severity");
            match store.set_issue_severity(project, number, severity.clone(), Some(me)).await {
                Ok(()) => done(format!("Raised #{number} to {}", issues::word(&severity)), format!("#{number} is {} now.", issues::word(&severity))),
                Err(refusal) => refused(format!("Changed severity of #{number}"), refusal),
            }
        }
        _ => ("refused", "Issues".into(), "Unknown action.".into(), result(false, "Unknown action.")),
    }
}
