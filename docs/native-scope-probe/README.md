# Native scope probe

This experiment tests restricted native-tool dispatch and compiles an owned
TextEdit adapter. Native actions have not been run. It is not connected to
MiniClaw's production worker, UI or model tools.

[`NativeScopePolicy`](../../scripts/experiments/native-scope-policy.swift) accepts
four operations: `observe`, `open`, `replaceText` and `save`. Trusted construction
supplies the document handles, file URLs, session/process binding, policy revision
and key. Requests cannot select an application, executable, PID, path or arbitrary
accessibility action.

The policy authenticates the exact payload bytes before decoding them:

```swift
        guard authentication.count == 32,
              HMAC<SHA256>.isValidAuthenticationCode(authentication,
                  authenticating: payload, using: key) else {
            return NativeReply(status: "unauthenticated")
        }
```

This proves possession of the injected key. No IPC listener, OS peer authentication
or production key-provisioning path is implemented. Tests and the manual smoke
harness supply their own temporary keys; they use no OAuth credentials.

Unknown fields, unsupported operations, unknown documents, read-only mutations,
stale policy/session/process identities and replayed sequences are refused before
dispatch. One operation may be in flight. The gate and sequence reservation happen
before awaiting the adapter:

```swift
        busy = true
        sequence = request.sequence
        let mutating = request.operation != .observe
        defer { busy = false }
```

Revocation or changed session state drops the returned observation. Failures after
a mutating call report `outcomeUnknown`; an edit already dispatched cannot be
undone by dropping its response. A revoked, changed or failed generation cannot
be reenabled through this object. Account provisioning and profile/process reset
are not implemented.

The [fake-adapter suite](../../tests/experiments/native-scope.test.mjs) verifies
dispatch, overlap, stale-result rejection and bounded outputs. It compiles
[`TextEditScopeAdapter`](../../scripts/experiments/native-scope-worker.swift) but
never constructs it. Those tests do not prove native confinement.

The native adapter uses the fixed system TextEdit bundle, actual worker UID/login
session and original process launch time. It checks trusted imported `.txt` file
identities and one matching document window. Its source uses AX text replacement,
a fixed native Save action, and a single-window ScreenCaptureKit image with audio
disabled. A session-switch observer permanently invalidates that adapter.

Document/window checks before and after asynchronous work are best-effort checks.
Transient changes may evade them. AX success acknowledges dispatch; it does not
prove persistence. TextEdit's actual AX support, save/atomic-replacement behavior,
file permissions and window matching still require the disposable native test.

[`native-scope-smoke.swift`](../../scripts/experiments/native-scope-smoke.swift)
provides a manual opt-in harness. Its `validate` mode performs file/identity checks
without native permission checks or actions. Other modes require the explicit
`--enable-native-smoke` flag and existing AX/Screen Recording permission. It never
requests those permissions, creates accounts or performs login. It rejects the
fixture owner and unexpected current UIDs. Standard-account status remains an
operator-verified prerequisite.

After dispatching Save, the harness checks the expected bytes at most five times,
100 ms apart. Unconfirmed bytes produce `saveUnconfirmed`, `outcomeUnknown: true`
and a failing exit. It does not retry the edit or Save action.
Edit mode refuses files already containing the target bytes and records the
initial SHA-256, so a previous edit cannot supply evidence for a new Save.

The smoke harness's read-only case verifies policy refusal; its denied-file check
uses OS access checks. Neither is a real native denied-file open/save attempt.
The [smoke tests](../../tests/experiments/native-scope-smoke.test.mjs) run only
disabled or validation refusal paths. Existing OAuth and broad Computer Use
integration remain separate from this experiment.

[Sequence](sequence.md)
