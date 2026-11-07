# Conversation search

The Search button below New chat in the sidebar and Cmd/Ctrl+K open a finder for user and assistant
messages across MiniClaw's visible conversations. Results carry the conversation summary,
message ID and a 240-character passage. Selecting one loads the conversation, follows older
history pages until the message is present, and scrolls to its highlighted container.

[Sequence](sequence.md)

## Index and matching

[`api/search.rs`](../../server/src/api/search.rs) owns `GET /api/search?q=…` and
`POST /api/search/refresh`. The first read starts a background pass through the existing
owned conversation listing and every history page. It does not resume conversations or
start model turns. Responses include `indexing`, `indexed`, `incomplete` and `limited`.
A missing provider, offline VM or failed history read sets `incomplete`; Refresh search
rebuilds the index. A backend restart also rebuilds on the first search.

[`search.rs::Index::new`](../../server/src/search.rs) opens a private SQLite temporary
index. It is separate from the chat cache and metadata database, uses an 8 MiB page cache,
can spill to disk, and disappears when its connection closes. SQLite work runs through
`spawn_blocking`, serialized by the index mutex. No new dependency is required.

```rust
let db = Connection::open("")?;
```

FTS5 trigrams find literal substrings. A dictionary of words and their single-character
deletions finds spelling candidates without scanning every message. A final comparison
accepts one insertion, deletion, substitution or adjacent transposition. Spelling correction
applies to alphanumeric words of 4–64 characters; shorter terms are literal. At least one
query term needs three characters. Query input is limited to 200 bytes and quoted before
it reaches FTS, so operators are literal text.

Literal matches precede approximate matches. All query terms must match. Results are limited
to 50 messages, with at most 1,000 candidates checked per literal/fuzzy pass and 64 dictionary
candidates per deletion key. `limited` tells the page to suggest a narrower query. Search
covers message text, not tool output, attachment contents or conversation names alone.

Completed chats still in the live cache are reconciled by their event cursor before a query.
The initial background pass reads full history, independently of the 400-message display
copies. External history changes and conversations created outside that cache need Refresh
search. Partial streaming replies are not continuously indexed.

## Ownership and page state

The index is scoped to the backend account generation and both providers' current identities.
Changing either identity replaces it. Queries filter removed conversations, hidden delegated
runs and signed-out providers, then apply current names and context. Old indexing jobs keep
only their old index and cannot publish progress into the replacement.

[`ConversationSearch.svelte`](../../web/src/ConversationSearch.svelte) uses a Bits UI dialog,
a 120 ms debounce and an AbortController per query. It polls every 500 ms only while indexing.
Closing the dialog or changing the query cancels that request. Arrow keys select; Enter opens;
Escape dismisses. Failures stay in the dialog. Opening a result does not send a message.

## Verification

`tests/search.test.mjs` covers unopened and paginated history, spelling, reuse, refresh,
completed responses, account changes, Claude history and deletion. `tests/browser/search.spec.ts`
checks an older-page match and Enter navigation on desktop and mobile.

The `search::tests::large_archive` debug test measured about 0.65 ms per warmed typo query
across 50,000 synthetic messages on the development Mac. This is an index measurement,
not end-to-end browser latency or a guarantee for arbitrary histories. Initial indexing and
opening a very old result still depend on provider history pagination.
