# Session ownership probe

This isolated macOS experiment verifies that a Node process can retain an advisory
file lock acquired by a short-lived native helper. It uses disposable files and
processes. Production VM startup and resource reconciliation do not call it.

Run `node --test tests/experiments/session-lock.test.mjs` with Node 24 and Apple's
Clang available. The [test harness](../../tests/experiments/session-lock.test.mjs)
compiles the [helper](../../scripts/experiments/session-lock.c), opens a private
regular file, and supplies its descriptor as child descriptor 3.

The `acquire` helper in the test transfers the open descriptor:

```js
    const result = spawnSync(binary, [], {
      stdio: ['ignore', 'pipe', 'pipe', fd], timeout: 5_000,
    });
```

The native `main` validates the descriptor's owner, type, link count and private
mode. It returns 0 when it acquires the lock, 3 for contention, and 2 for an invalid
descriptor or another system failure. It accepts no path and opens no files.

```c
    if (flock(3, LOCK_EX | LOCK_NB) != 0) {
        return errno == EWOULDBLOCK ? 3 : 2;
    }
    // Explicit unlock would also release the parent's shared lock.
    return 0;
```

Node keeps its original descriptor after the helper exits. An independently opened
descriptor remains unable to acquire that inode's lock until the final shared
reference closes. An ordinary child does not inherit the descriptor; an explicitly
supplied copy keeps the lock alive. The tests also kill a disposable owner process
and verify that another process can acquire its former lock afterward.

Renaming the locked file and creating a replacement at its old path does not move
ownership to the replacement. The test verifies different device/inode identities,
then confirms the original inode remains locked.

These observations cover advisory lock ownership on this host. They do not prove
that a native virtual machine has stopped or authorize releasing a resource
reservation. The [resource admission](../resource-admission/README.md) code retains
unknown reservations.

Apple documents shared lock references for duplicated descriptors and release on
the final close. Node documents supplying a parent descriptor through child `stdio`.
See [flock](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html),
[close](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/close.2.html),
and [Node child processes](https://nodejs.org/docs/latest-v24.x/api/child_process.html#optionsstdio).

[Sequence](sequence.md)
