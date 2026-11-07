# Recovery evidence probe

This isolated macOS experiment measures disposable file identities, publishes
bounded fixture receipts and classifies injected recovery evidence. It does not
open a resource ledger, start a VM or release a reservation.

Run Node 24 with
`node --test tests/experiments/recovery-proof.test.mjs tests/experiments/session-lock.test.mjs`.
The [harness](../../tests/experiments/recovery-proof.test.mjs) compiles the native
[file helper](../../scripts/experiments/file-evidence.c) with Apple's Clang. It
passes one already-opened fixture descriptor as fd 3. The helper accepts only
`inspect`, `sync-file` or `sync-directory`; it opens no paths.

`inspect` reports device, inode and `st_gen` from `fstat`. The tests compare the
first two with Node's metadata, rename the original file and create a replacement,
and verify that the retained descriptor still names the original inode. A zero
or nonzero `st_gen` measurement alone is not proof of a persistent immutable file
identity. The test reports the value without enabling recovery.

For regular files, the helper requires a private, caller-owned file with one link.
For directory flushes, it requires a private caller-owned directory. Its native
`main` uses these separate synchronization operations:

```c
    if (strcmp(argv[1], "sync-directory") == 0) {
        if (!S_ISDIR(metadata.st_mode)) return 2;
        return fsync(3) == 0 ? 0 : 2;
    }
```

```c
    if (strcmp(argv[1], "sync-file") == 0) {
        return fcntl(3, F_FULLFSYNC) == 0 ? 0 : 2;
    }
```

[`publishReceipt`](../../scripts/experiments/recovery-proof.mjs) writes at most
4096 JSON bytes into a new private staging file. Its injected sync callbacks let
the tests use the native helper or fail at a chosen boundary. It flushes the file,
checks the directory still names the opened object, exclusively links the staging
file to its final name, removes the staging name, then syncs the directory.
An existing final receipt is never overwritten.

```js
    linkSync(staged, final);
    unlinkSync(staged);
    syncDirectory(dir);
    return final;
```

A file-sync failure leaves no final receipt. A directory-sync failure throws but
can leave a complete final file. The suite also makes owned publisher processes
exit at those boundaries. A later process cannot infer completed synchronization
from that visible file. These tests establish syscall results and process-failure
behavior, not survival of power loss. The publisher is a cooperating-fixture
experiment; its directory recheck is not a complete hostile-path race defense.

`classifyRecovery` accepts synthetic lease, attempt, role, identity and held-lock
fields. A valid prepared phase is classified `never_started`. Spawn-intent evidence
requires an explicitly confirmed publication and a matching versioned native
receipt. Successful guest stop and forced stop remain distinct classifications.
Unknown identities, held-lock failures, stale bindings, failed native stop or
unconfirmed publication return `unknown`.

Every classification contains `productionRelease: false`. These input fields are
injected assertions, not live locks, authenticated callbacks or durable proof.
No code translates the helper's `st_gen` into the classifier's identity string.
There is no production recovery entry point.

The [session ownership probe](../session-ownership-probe/README.md) separately
verifies shared-descriptor lock lifetime. Resource admission continues to retain
[unknown reservations](../resource-admission/README.md).

Apple describes [stat generation fields](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/stat64.2.html)
and [file synchronization](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html).
Foundation's [generationIdentifier](https://developer.apple.com/documentation/foundation/urlresourcevalues/generationidentifier)
changes with file data or directory children; it is not the stable identity of a
running writable VM disk.

[Sequence](sequence.md)
