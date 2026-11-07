# VM connection

MiniClaw's backend runs on the Mac and owns the API, SQLite data and UI. A VM is an
optional executor for untrusted project work: the macOS guest owns its own Codex, OAuth
storage and workspace. Host JavaScript opens the private sockets and owns the filtered
network helper. No VM is needed for normal chat. See the [sequence](sequence.md).

> Measured results further down this page were taken under the earlier layout, where a
> Linux VM hosted the backend. The guest side is unchanged; the Mac-backend path has not
> been run against a real VM yet.

## Startup and ownership

[startPair](../../scripts/vm/session.mjs) requires a Mac-backend pair (`pair.mac`). It
claims `ActiveSession` beneath the private installation directory, creates private
temporary Unix-socket directories, starts the brokers, the control socket and the egress
proxy, and boots nothing. It reserves no resources at start. Existing session ownership
is refused. Crashed session locks are not reclaimed automatically.

```js
// scripts/vm/session.mjs, startPair
await writeFile(join(hostDir, 'links.json'), JSON.stringify({
  controlSocket: join(hostDir, 'control.sock'), controlKey: controlKeyPath,
  worker: pair.mac.worker, master: join(dir, 'controller.sock'), dedicated: dir,
}), { flag: 'wx', mode: 0o600 });
```

[start-host.mjs](../../scripts/start-host.mjs) passes that file to the backend as
`MINICLAW_VM_LINKS` and closes the session after the backend exits. The backend reads it
with `remote::Links`, runs the Mac's own Codex behind the worker protocol in-process so
it also answers `miniclaw/vm/*` from the control socket, and dials each VM worker's Unix
socket: `master` for the main VM and `dedicated/dNN/controller.sock` for project VMs.
A missing socket means that VM is off.

The main VM boots when a task asks: the backend's start route calls the control socket,
which calls `startMaster`, which reserves that VM's share and starts its supervisor.

The native [GuestBridge](../../scripts/vm/guest-bridge.swift) accepts connections only
from its own guest, on virtual port `10242` for RPC and `10243` for egress. These are
virtual socket numbers, not host TCP listeners.

[startBroker](../../scripts/vm/broker.mjs) pairs one backend stream with one worker
stream. A disconnect closes both. Unpaired streams have a ten-second timeout. The
[worker protocol](../worker-transport/README.md) authenticates peers before starting App
Server. The host broker is trusted and can observe forwarded bytes.

Shutdown signals request graceful guest shutdown. Native supervisors use
[HeadlessSession](../../scripts/experiments/vm-headless.swift): the supervisor sends a stop request, then
presses Return in the guest to confirm the shutdown dialog macOS shows for it. After 90
seconds without confirmation, it calls native power-off. Repeated requests do not reset the timer. An
early stop cancels it.

Forced power-off can lose unsaved data and unflushed writes. It emits `force_stopping`,
then `forced_stopped` and exit code `2` after confirmation. A failed power-off emits
`force_stop_failed` and keeps ownership until a confirmed stop; another shutdown request
retries. Socket cleanup waits for every native supervisor to exit. Disks are retained.

A VM that fails shows in its status: `masterStatus` reports `failed` with a reason such
as `Master VM network disconnected.` The session itself ends only when it is closed.

## The VM window

A VM started in normal use runs hidden with a view-only window
([vm-window.swift](../../scripts/experiments/vm-window.swift)). Until the VM's worker
connects, the window accepts input, and after 25 seconds without a worker it shows itself
with "Log in to this VM to continue". When the worker connects, the window announces
"Logged in. This window hides in a moment" for five seconds, hides, and shields its
display from input. A **Control this VM** switch in the window's bar lifts that shield;
it is how the user grants macOS permissions inside the guest. The live picture in the chat
page is described in [VM desktop stream](../desktop-stream/README.md).

## Tool calls inside a VM

Every VM chat gets `miniclaw_scoped_native`, whether or not it belongs to an agent or a
project: the VM is the sandbox ([host_tools](../../server/src/api.rs)). On the Mac the tool
still requires an agent or a project. The model is told that it runs inside a MiniClaw VM
and where the VM's workspace folder is; that folder needs no approval. A chat with
Computer Use on does not get this tool, because the VM's worker refuses both tool sets
together (see [Computer Use](../computer-use/README.md)).

A VM chat's `miniclaw_scoped_native` calls go to that VM's own worker with the
conversation's grants. The worker runs them through the same
[executor](../../server/src/executor.rs) and Seatbelt profile as the Mac, and Codex in
the guest never sees the frame:

```rust
// server/src/worker.rs, requests
if frame["method"] == "miniclaw/scoped/execute" {
```

AppleScript and TextEdit operations are refused on every machine.

## Guest runtime

[prepareMacVm](../../scripts/vm/prepare-mac.mjs) generates a random per-installation key and
places it in the Mac's private `worker.json` (mode `0600`) and on the read-only Master
installation media.
The key is separate from OAuth credentials and never appears in browser responses.
Installation files still contain sensitive pairing material and must remain private.

[Settings::read](../../server/src/remote.rs) rejects linked, oversized, non-private
or foreign-owned worker settings. The Linux controller uses the guest workspace
path without creating that macOS path locally. It attempts the worker connection
four times and exits on failure; systemd supplies service restart handling.
After connection, `remote::guard` ends the controller with an error if the worker
transport closes. Explicit shutdown is tracked separately and finishes normally.
Requests are not replayed across the restart. Unit tests cover both exit paths;
retained worker replacement recovered in2240ms without rebooting either VM.

[The guest installer](../../scripts/vm/install-master.command) installs the verified
Codex package, a content-addressed worker executable and a user LaunchAgent. The
worker starts at login as the guest user. The VirtualMac check prevents accidental
host installation; it is not a hostile-caller security boundary.

[miniclaw-worker](../../server/src/bin/miniclaw-worker.rs) uses the installed guest
Codex path and validates its workspace. [worker_command](../../server/src/runtime.rs)
checks the provider and disables inherited MCP servers before returning the command.
Its child `PATH` starts with the installed Codex directory, then fixed macOS system
directories; it does not depend on shell profiles.
The worker owns App Server and reaps it when the authenticated connection ends.

## HTTPS egress

Codex receives guest-loopback proxy settings. The worker forwards these connections
through virtual port `10243` to [startEgress](../../scripts/vm/egress.mjs). The proxy
accepts CONNECT on port `443`. Paired startup defaults to `public-web`; setting
`pair.egress` to `openai` restricts it to the names below, from `egress.mjs`:

```js
const hosts = new Set(['auth.openai.com', 'chatgpt.com', 'api.openai.com']);
```

DNS resolves to public IPv4 addresses and excludes addresses assigned to the host.
The proxy dials the checked numeric address, preventing a second DNS lookup. It
does not intercept TLS. `public-web` accepts public DNS names, excluding local and
reserved suffixes. Both modes reject IP literals, private addresses, malformed
names and other ports. Unknown modes fail before any VM starts.

The guest proxy admits 16 connections; the host proxy admits 32. HTTP headers are
bounded at 8 KiB, DNS at five seconds and tunnel inactivity at 120 seconds. Only
proxy-configured clients use this path. The Linux guest has no network adapter;
neither guest has a host folder share.

[The live proxy test](../../tests/experiments/public-egress.test.mjs), explicitly
enabled with `MINICLAW_PUBLIC_EGRESS=1`, reached `example.com` with certificate
verification and rejected local destinations. This tests the host proxy, not a
browser inside the Master VM. Older installations retain their packaged policy
until their runtime files are updated.

## Master network

When `pair.network` names the installed helper, [startPair](../../scripts/vm/session.mjs)
starts it before the guests. [attachNetwork](../../scripts/vm/guest-network.swift)
connects the Master's NIC to a private Unix datagram socket. It does not create a
bridged adapter, host TCP listener or shared network between guests.

[The helper](../../scripts/vm/network-helper/network.go) uses `gvisor-tap-vsock`
for DHCP, DNS and TCP/IP handling. MiniClaw does not serve its management API or
configure port forwarding. [The packet policy](../../scripts/vm/network-helper/policy.go)
admits public IPv4 TCP/UDP and the private gateway's DNS/DHCP services. It rejects
host interface addresses, private/reserved destinations, IPv6, fragments, IP options
and spoofed guest identities. ARP replies may resolve the guest for public peers;
they cannot claim another source IP or MAC.

`sendFrame` drops packets when the receive buffer is full or its 20 ms write deadline
expires. Other transport errors end the helper; the paired launcher shuts down its
owned guests. Both datagram endpoints use4MiB receive and1MiB send buffers. A native
NIC disconnection reports `network_failed`, invalidates readiness and ends the pair
without enabling a fallback network. Packet loss can require retransmission.
This is not an inbound firewall
for applications running directly on the host, nor a per-agent domain policy.
Public outbound access also permits uploads and outbound tunnels. Destination
filtering does not prevent exfiltration through an allowed public service.

[prepareMacVm](../../scripts/vm/prepare-mac.mjs) builds the helper with the reviewed,
checksum-verified Go archive and locked module dependencies. The wizard includes
the archive and extraction/cache allowance in its installation review. Go is installed
inside the installation directory, not system-wide.

`npm run test:network` runs packet-policy, DNS, buffer-pressure and local-canary tests
with Go on `PATH`. Set `MINICLAW_NETWORK_LIVE=1` to include public DNS resolution.
These are helper tests. Installed guest checks also passed HTTP, certificate-verified
HTTPS and Safari rendering against example.com; a host-LAN request timed out.
These checks do not establish that every Apple Account service works.

## Wizard authentication

[connectSetup](../../scripts/setup/connect.mjs) starts the session, starts the installed
backend with the link file, boots the main VM once, and waits up to 30 minutes for the
manual setup confirmation and for `GET /api/vms/master/status` to report `ready`. VM
routes need one provider signed in on the Mac, so the wizard asks for that first when
needed. It then calls [signInGuest](../../scripts/setup/auth.mjs), which starts device
OAuth through `POST /api/vms/master/auth/start`. That route uses the guest's own worker,
never the Mac's Codex. The wizard shows the verified OpenAI URL and code and polls the
status for `signedIn`. It does not copy host credentials.

Authentication has a 15-minute deadline and 20-second request deadlines. A failure stops
the backend first, then the VMs. `miniclaw setup --resume --destination PATH` resumes
this stage from saved settings.

## Verification

The retained pair completed guest OAuth, streamed real Astra replies and reused
sign-in through the actual setup-resume wizard. The installed `miniclaw start` path
also connected and streamed a reply after correcting the lab's service diagnostics
and relay dependency. Earlier startup timeouts remain recorded in `TESTING.md`;
one successful retained-guest boot is not a fresh-install timing result.
The fresh-directory wizard subsequently completed image preparation, macOS install
and guest OAuth using verified cached archives. Real browser chat passed, followed
by two installed starts with retained sign-in and restored conversation. Readiness
took 40,045 ms and 82,404 ms including manual unlock, within the 126-second deadline.
Clean-host dependency bootstrap was not tested.
[Guest Computer Use](../computer-use/README.md#guest-execution) now has explicit
worker-side configuration and optional signed installation media. Guest Calculator
capture/input and an offline Safari interaction passed after guest permissions were
granted. Scoped native app/file restrictions remain unavailable.
