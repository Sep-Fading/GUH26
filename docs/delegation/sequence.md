# Delegated progress and opening a run

[Delegation](README.md)

```mermaid
sequenceDiagram
    participant User
    participant UI as DelegationBlock / Chat
    participant API as Delegation API
    participant Store as Run store
    participant Child as Child conversation
    participant Parent as Parent model
    User->>UI: Show the full run
    loop While expanded and running
        UI->>Child: GET snapshot
        Child-->>UI: Partial messages and pending approval or review
    end
    User->>Parent: What is the progress on that task?
    Parent->>API: miniclaw_delegate action=status
    API->>API: Check active calling turn
    API->>Store: recent_runs(parent, optional runId)
    Store-->>API: Owned run records
    API->>Child: Read latest update and bounded activity metadata
    API-->>Parent: Status, waits, updates and result
    Parent-->>User: Explain observed progress
    User->>UI: Open conversation with agent
    UI->>API: POST detach
    API->>Store: Mark existing run detached
    API-->>UI: Existing child thread ID
    UI->>Child: Read snapshot and select same conversation
    Note over Store,Child: Original task keeps its own limits and restrictions until it ends
```

| Participant | Source |
| --- | --- |
| DelegationBlock / Chat | [DelegationBlock.svelte](../../web/src/DelegationBlock.svelte): refresh effect and `openConversation`; [chat.svelte.ts](../../web/src/chat.svelte.ts): `runTranscript`, `continueWithRun` |
| Delegation API | [api/delegation.rs](../../server/src/api/delegation.rs): `handle`, `status`, `detach`, `attempt` |
| Run store | [delegations.rs](../../server/src/delegations.rs): `recent_runs`, `detach_run`, `is_delegated` |
| Child conversation | [chat.rs](../../server/src/chat.rs): `snapshot`, `publish_activity`; activity metadata from [activity.rs](../../server/src/activity.rs): `list` |
| Parent model | Existing Codex dynamic-tool dispatch in [chat.rs](../../server/src/chat.rs); Claude bridge in [tool_bridge.rs](../../server/src/tool_bridge.rs) |

## Approval from another conversation

```mermaid
sequenceDiagram
    participant A as Requesting agent
    participant B as Backend
    participant U as Page viewing another chat
    A->>B: Request delegation
    B->>B: Store prompt with source and deadline
    U->>B: GET threads/working
    B-->>U: delegationPrompts
    U->>U: Show approval and source conversation
    U->>B: Answer source thread and prompt ID
    B->>B: Validate ownership, ID and deadline
    B-->>A: Apply explicit user choice
    U->>U: Keep current chat and draft
```

Sources: api.rs `working_tasks`, api/delegation.rs `answer`, chat.svelte.ts
`refreshWork` and `answerDelegation`. Diagram not visually rendered.


## Child requests in the parent

```mermaid
sequenceDiagram
    participant Child as Child conversation
    participant Loop as Delegation run loop
    participant UI as Parent Chat UI
    participant API as Existing decision routes
    Child->>Child: Hold permission, computer, review or question request
    Loop->>Child: Read live requests for this run
    Loop-->>UI: delegation.requests with run ID and child owner
    UI->>API: Read parent snapshot or working-tasks poll on return
    API-->>UI: Current delegatedRequests
    UI->>UI: Keep own prompts and each child's queue separate
    UI->>API: User answers at child thread request URL
    API->>Child: Validate active turn and deadline, deliver answer
    API-->>UI: Acknowledge decision
    UI->>UI: Remove this request, display next
    Loop-->>UI: Updated queue for this run
```

Source: [`pending_requests`, `attempt`, `describe`](../../server/src/api/delegation.rs),
[`working_tasks` and decision routes](../../server/src/api.rs),
[`Chat` pending getters and decision methods](../../web/src/chat.svelte.ts).
Diagram syntax inspected; no Mermaid renderer was run.
