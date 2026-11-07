# Creation recovery

Explicit recovery frees creation-journal slots without identifying or admitting any
chat whose creation outcome is unknown. Open **Notifications**, then **Creation
recovery**. Review the unknown and known receipt counts, check the acknowledgement
and choose **Free creation capacity**. [Sequence](sequence.md).

## Scope and result

The [preview route](../../server/src/api.rs), `creation_recovery`, returns counts
and a confirmation token. The confirmed POST returns the number of released slots.
Both require current Codex sign-in and the existing management-origin guard. This
is installation-wide recovery: the metadata store and existing creation denials
are shared across account changes, rather than being filtered to one account.

Compaction does not delete Codex chats, match receipts from transcript text or send
messages. Existing admitted chats keep their bindings. Receipts with a saved Codex
thread ID remain in the ordinary [binding recovery](../agents/README.md#recovering-interrupted-creation)
path. Unknown creations lose their provisional agent/project/skill copies only when
the equivalent permanent denial has been written in the same transaction. The saved
agent, project and skill definitions are retained.

The [interface](../../web/src/CreationRecovery.svelte) displays a fresh preview and
requires an unchecked acknowledgement before confirmation. It clears confirmation
after a response, error, refresh or account change. A stale token requires another
preview and confirmation. Failed or lost responses instruct the user to refresh;
there is no automatic recovery retry or message resend.

## Permanent denial

Before compaction, a chat without an admission record is allowed only if every
unknown creation's baseline includes its ID. [Quarantine storage](../../server/src/agents/quarantine.rs)
retains the intersection of those baselines. Later compactions intersect with the
previous quarantine baseline as well. An empty intersection remains an active
fence that denies every unadmitted ID. A new creation with a committed admission
remains usable under its recorded policy.

`compact_creations` preserves that rule directly:

```rust
                preserved = Some(match preserved {
                    None => intent.baseline.clone(),
                    Some(existing) => existing.intersection(&intent.baseline).cloned().collect(),
                });
```

The mandatory `creation_quarantine` row has `singleton = 1`, an integer revision
and a nullable baseline. Revision zero requires a null baseline and means no
compaction has occurred. Positive revisions require a sorted, unique array of at
most 4096 canonical UUID strings. An empty array is valid and differs from null.
Missing state, invalid JSON, duplicate IDs and inconsistent revision/null state
fail closed. Revision overflow also refuses recovery.

[Schema initialization](../../server/src/agents/creation.rs) upgrades version 1 to
version 2 in a transaction, adding exactly one inactive quarantine row while
preserving all existing receipts and admissions. Version 2 validates that state
and never recreates a missing table or row. Database and companion-file identity
checks remain in the [agent store](../../server/src/agents.rs). These checks detect
missing or invalid state; they do not authenticate every database value against
coordinated edits by a process able to modify private metadata.

## Confirmation and atomicity

The confirmation token hashes the fence revision and baseline plus ordered journal
IDs, known receipt IDs and the exact stored baseline text. The POST takes
`create_gate`, `agent_gate` for writing and `load_gate`, matching creation's lock
order. It refreshes sign-in and holds the account-generation lock during compaction.
An in-flight creation finishes first; a changed journal rejects the stale token.

The SQLite transaction validates required provisional records, computes the
intersection, updates the singleton, rereads the exact expected result, then removes
only the confirmed unknown intents and their provisional rows. A failed, ignored
or changed quarantine update cannot release the journal:

```rust
            if changed != 1 || stored.revision != revision || stored.baseline.as_ref() != Some(&preserved) {
                return Err(Issue::Storage);
            }
```

A failed cleanup rolls back both the new fence and every deletion. Known receipts
are untouched. Confirmed chat deletion remains independent and cannot remove the
quarantine fence.

The 64 pending-creation limit can be relieved through this workflow. The separate
4096-ID admission-baseline bound is unchanged. Storage faults and a journal containing
only known receipts require their existing repair/retry path; compaction does not
bypass them. A missing quarantine singleton requires diagnosis, not recreation or
a metadata reset.

## Verification

[API acceptance](../../tests/creation-recovery.test.mjs) passes nine cases covering
full-journal release, monotonic and empty intersections, known-receipt preservation,
stale tokens, account/origin guards, rollback, ignored writes, held creation and
actual agent/project/skill provisional cleanup. Existing
[creation-storage acceptance](../../tests/storage-errors.test.mjs) passed nine cases.

[Schema tests](../../server/tests/creation_recovery.rs) pass three cases for retained
version-1 migration, missing/corrupt singleton state and revision overflow.
[Component tests](../../web/tests/creation-recovery.test.ts) pass three cases for
explicit acknowledgement, stale confirmation and an account change during preview.
[Browser tests](../../tests/browser/creation-recovery.spec.ts) pass desktop and mobile
flows with draft preservation, no resend, restart, nested dismissal and 320px width.
All fixtures use disposable data and synthetic Codex peers.
