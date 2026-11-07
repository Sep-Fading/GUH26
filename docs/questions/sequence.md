# Questions from the model: sequence

Back to [Questions from the model](README.md).

```mermaid
sequenceDiagram
    participant M as Model (Codex or Claude)
    participant H as api/ask.rs handle
    participant C as Chat::question
    participant P as Page (QuestionDialog)
    participant U as User
    participant I as api.rs interrupt

    M->>H: item/tool/call miniclaw_ask {questions}
    H->>H: parse, check the no_questions switch
    H->>C: store the question, emit snapshot
    C-->>P: snapshot.question
    P-->>U: popup with options and "Your own answer"
    alt the user answers
        U->>P: Send answers
        P->>H: POST /questions/{id} {answers}
        H-->>M: success, each question with its answer
    else "Answer in chat"
        U->>P: Answer in chat
        P->>H: POST /questions/{id} {action: chat}
        H->>I: interrupt the turn
        H-->>M: refused
    else nobody answers for 30 minutes
        H->>I: interrupt the turn
        H-->>M: refused
    end
    H->>H: publish_activity kind question
```

| Participant | Source |
|---|---|
| handle | [`api/ask.rs`](../../server/src/api/ask.rs) `install`, `handle`, `answer`, `switched_off` |
| Chat::question | [`chat.rs`](../../server/src/chat.rs) `PendingQuestion`, `QuestionReply`, the `turn/completed` branch that drops it |
| Page | [`QuestionDialog.svelte`](../../web/src/QuestionDialog.svelte), [`chat.svelte.ts`](../../web/src/chat.svelte.ts) `answerQuestion` |
| Tool and checks | [`ask.rs`](../../server/src/ask.rs) `tool_definition`, `parse`, `answers_fit`, `result_text` |

The diagram's syntax was not checked with a renderer.
