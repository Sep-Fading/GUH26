//! The pending question of a conversation: the tool call that raises it, the route that answers
//! it, and the wait. Nobody answering ends the turn, as the user decided; it is never left
//! waiting and the model is never told to carry on without an answer.

// Reuses interrupting and the shared request checks.
use super::*;
// The question's shape, its checks and the words the model receives.
use crate::ask::{self, Answer};
// The pending question kept on the conversation.
use crate::chat::{PendingQuestion, QuestionReply};
// Times how long a question waits.
use std::time::Instant;

/// How long a question waits for the user: 30 of the minutes tests can shorten.
fn wait() -> Duration { super::schedules::minute() * 30 }

/// An answer for the model: success or not, and words.
fn result(success: bool, text: &str) -> Value {
    json!({ "success": success, "contentItems": [{ "type": "inputText", "text": text }] })
}

/// Reports whether asking is switched off for this conversation. It is on unless the switch is set.
pub(super) async fn switched_off(backend: &Backend, thread: &str) -> bool {
    use crate::permissions::{Decision, Target};
    let Ok(context) = backend.agents.permission_context(thread.to_owned()).await else { return false };
    matches!(backend.agents.permission(&context, &Target::Feature { name: "no_questions".into() }, chrono::Utc::now().timestamp()).await,
        Ok(Decision::Allow(_)))
}

/// Registers the tool's handler with the notification loop.
pub(super) fn install(backend: &Shared) {
    let _ = backend.ask.set(Box::new(|backend, request| Box::pin(async move {
        let params = request.params.clone();
        // The wait for the user is the step's duration.
        let started = chrono::Utc::now().timestamp_millis();
        let (status, answer, output) = handle(&backend, &params).await;
        if let (Some(thread), Some(id), Some(turn)) = (params["threadId"].as_str(), params["callId"].as_str(), params["turnId"].as_str()) {
            let title = ask::parse(&params["arguments"]).map(|questions| questions.iter().map(|question| question.question.clone())
                .collect::<Vec<_>>().join(" / ")).unwrap_or_else(|| "A question MiniClaw could not read".into());
            let now = chrono::Utc::now().timestamp_millis();
            backend.publish_activity(thread, &json!({ "id": id, "turnId": turn, "kind": "question", "title": title, "detail": null,
                "agent": null, "status": status, "startedAt": started, "endedAt": now.max(started + 1), "output": output })).await;
        }
        let _ = request.answer.send(answer);
    })));
}

/// Shows the question and waits for the user. Answers the Activity status, what the model
/// receives, and what Activity shows.
async fn handle(backend: &Shared, params: &Value) -> (&'static str, Value, String) {
    let refused = |text: &str| ("refused", result(false, text), text.to_owned());
    let (Some(thread), Some(turn)) = (params["threadId"].as_str(), params["turnId"].as_str())
        else { return refused("MiniClaw could not tell which conversation this question belongs to.") };
    let Some(questions) = ask::parse(&params["arguments"]) else {
        return refused("MiniClaw could not read this request, so nothing was asked. Send 1 to 4 questions, each with a question and 2 to 4 options that have distinct labels.");
    };
    if switched_off(backend, thread).await {
        return refused("The user switched questions off for this conversation. Decide with your best judgement and say what you assumed.");
    }
    let (sender, answered) = tokio::sync::oneshot::channel();
    let id = Uuid::new_v4();
    // A run nobody watches waits as long as its schedule allows for approvals.
    let unattended = backend.unattended.lock().ok().and_then(|runs| runs.get(thread).copied());
    let window = unattended.unwrap_or_else(wait);
    {
        let mut data = backend.data.lock().await;
        let Some(chat) = data.chats.get_mut(thread) else { return refused("This conversation is not open.") };
        if chat.stopping || chat.active.as_deref() != Some(turn) || chat.question.is_some() {
            return refused("A question is already waiting, or the response is ending.");
        }
        chat.question = Some(PendingQuestion { id, turn: turn.into(), questions: questions.clone(),
            expires: Instant::now() + window, answer: sender });
        chat.emit("snapshot", chat.snapshot());
    }
    backend.changed();
    let reply = tokio::time::timeout(window, answered).await;
    {
        let mut data = backend.data.lock().await;
        if let Some(chat) = data.chats.get_mut(thread) && chat.question.as_ref().is_some_and(|open| open.id == id) {
            chat.question = None;
            chat.emit("snapshot", chat.snapshot());
        }
    }
    backend.changed();
    match reply {
        Ok(Ok(QuestionReply::Answers(answers))) => {
            let text = ask::result_text(&questions, &answers);
            ("ok", result(true, &text), text)
        }
        Ok(Ok(QuestionReply::InChat)) => {
            let _ = interrupt(State(backend.clone()), Path(thread.to_owned()), Ok(Json(Interrupt { turn_id: turn.to_owned() }))).await;
            refused("The user chose to answer in the chat instead, so this response was stopped.")
        }
        // The turn ended or the conversation was closed while the question was open.
        Ok(Err(_)) => refused("The question was withdrawn because the response ended."),
        Err(_) => {
            if unattended.is_some() && let Ok(mut expired) = backend.unattended_expired.lock() {
                expired.insert(thread.to_owned(), format!("a question: {}", questions[0].question));
            }
            let _ = interrupt(State(backend.clone()), Path(thread.to_owned()), Ok(Json(Interrupt { turn_id: turn.to_owned() }))).await;
            refused("The user did not answer in time, so this response was ended.")
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reply { #[serde(default)] answers: Option<Vec<Answer>>, #[serde(default)] action: Option<String> }

/// Takes the user's answers, or their choice to answer in the chat instead.
pub(super) async fn answer(State(backend): State<Shared>, Path((id, question)): Path<(String, String)>, input: Body<Reply>)
    -> Result<Json<Value>> {
    let input = body(input)?;
    authorized_thread(&backend, &id).await?;
    let mut data = backend.data.lock().await;
    let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
    let open = chat.question.as_ref().filter(|open| open.id.to_string() == question && open.current()).ok_or_else(approval_gone)?;
    let reply = match (input.answers, input.action.as_deref()) {
        (Some(answers), None) if ask::answers_fit(&open.questions, &answers) => QuestionReply::Answers(answers),
        (None, Some("chat")) => QuestionReply::InChat,
        _ => return Err(invalid()),
    };
    let open = chat.question.take().expect("checked above");
    open.answer.send(reply).map_err(|_| approval_gone())?;
    chat.emit("snapshot", chat.snapshot());
    drop(data);
    backend.changed();
    Ok(Json(json!({})))
}
