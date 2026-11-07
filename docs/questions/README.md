# Questions from the model

A model can put a question to the user and wait for the answer. The tool is `miniclaw_ask`
([`ask.rs`](../../server/src/ask.rs)): 1 to 4 questions, each with 2 to 4 options, one pick or
several, and always a field for the user's own words. The answers return as the tool's result.
Asking grants nothing; the tool can only show a question and wait.

Sequence: [sequence.md](sequence.md).

## The tool

| Field | Rule |
|---|---|
| `questions` | 1 to 4 |
| `question` | required, 600 bytes at most |
| `header` | optional short label, 40 bytes at most |
| `multiSelect` | optional; several options may be chosen |
| `options` | 2 to 4, with distinct `label` (80 bytes) and optional `description` (300 bytes) |

Its description tells the model when to use it: a decision that is the user's and changes what
happens next, with the recommended option first and marked "(Recommended)", never for facts it
can find out itself, and without an "Other" option, since the user can always write their own
answer. A request outside the shape asks nothing and is explained to the model.

It is offered to every conversation on this Mac that does not use Computer Use
([`api.rs`](../../server/src/api.rs) `offer_ask`). A VM conversation gets it when its worker
carries it; the popup, the wait and the switch are the Mac's. See [worker transport](../worker-transport/README.md#tools-the-mac-answers). A delegated run does not get it either, because its question
would land in a conversation the user may not be looking at.

## Waiting and answering

The handler ([`api/ask.rs`](../../server/src/api/ask.rs)) stores the question on the
conversation as `Chat::question`, which the snapshot carries, and waits:

```rust
match reply {
    Ok(Ok(QuestionReply::Answers(answers))) => {
        let text = ask::result_text(&questions, &answers);
        ("ok", result(true, &text), text)
    }
    Ok(Ok(QuestionReply::InChat)) => {
        let _ = interrupt(State(backend.clone()), Path(thread.to_owned()), Ok(Json(Interrupt { turn_id: turn.to_owned() }))).await;
        refused("The user chose to answer in the chat instead, so this response was stopped.")
    }
```
[`api/ask.rs`](../../server/src/api/ask.rs) `handle`

`POST /api/threads/{id}/questions/{question}` takes either `answers`, one per question with
`selected` labels and optional `other`, or `action: "chat"`. Answers must fit: only offered
labels, one pick unless several are allowed, and every question answered by a pick or by the
user's own words (4000 bytes at most). Anything else answers 400 and the question stays open.

- **Answered:** the model receives each question with what was chosen and written.
- **"Answer in chat":** the turn is interrupted, so the user replies with an ordinary message.
- **No answer in 30 minutes:** the turn is interrupted. The model is not told to carry on
  without an answer; that was the user's decision. A scheduled run that nobody watches waits as
  long as its schedule allows for approvals and is then terminated, like an approval.
- **The turn ends for any other reason:** the question is dropped with it.

One question is open per conversation at a time. Every question is an
[activity](../activity/README.md) entry of kind `question`, with the answers as its output.

## Switching it off

Asking is on by default. "Ask me questions" in the composer's Tools dropdown switches it off
for one conversation ([capabilities](../capabilities/README.md)). It is stored inverted, as
`feature no_questions`, like web search, and needs no agent's permission. With it off the tool
answers that the user switched questions off and that the model should decide and say what it
assumed. A Claude turn carries the tool only while asking is on (`miniclawAsk` on `turn/start`).

Because asking is on by default, a plain Claude turn now carries MiniClaw's shim with this one
tool and no file tool (`MINICLAW_NO_FILE_TOOL`).

## The popup

[`QuestionDialog.svelte`](../../web/src/QuestionDialog.svelte) is a modal that neither Escape
nor a click outside closes. Each question is a group named by the question, with its options as
radio buttons or checkboxes on cards, and a "Your own answer" field. "Send answers" is enabled
once every question has a pick or the user's words. It waits behind an approval popup when both
are open.

## Tests

[`questions.test.mjs`](../../tests/questions.test.mjs) (5, both fake providers),
[`questions.spec.ts`](../../tests/browser/questions.spec.ts) (3, with an axe scan of the popup),
and unit tests in [`ask.rs`](../../server/src/ask.rs).

Observed with a real Codex model on 2026-09-21: it opened the popup for a decision that was the
user's and carried on with an answer in the user's own words ([briefing](../briefing/README.md)).
Not observed: Claude, whether models ask at sensible moments across ordinary work, and how
either provider behaves while a tool call stays open for many minutes.
