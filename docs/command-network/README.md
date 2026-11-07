# Network for shell commands

A shell command run through `miniclaw_scoped_native` has no network. When network is
switched on for the conversation and the user approves the hosts a command names, that one
command may open HTTPS connections to exactly those hosts. Nothing else changes: the command
still runs in its Seatbelt profile and still cannot reach the Keychain, a provider's sign-in
files or MiniClaw's own state.

See the [sequence](sequence.md).

## Naming hosts

`run_shell` takes an optional `hosts` list of at most eight distinct DNS names
([Operation::parse](../../server/src/scoped.rs)). Each becomes a permission target next to
the command's own:

```rust
// server/src/scoped.rs, Operation::targets
}).chain(hosts.iter().map(|host| Target::Host { host: host.clone() })).collect(),
```

A host is a plain lowercase DNS name with at least two labels
([dns_name](../../server/src/permissions.rs)). Addresses, ports, paths, wildcards and single
labels such as `localhost` are refused.

## The switch

Network is a switch stored as a permission rule, `Target::Feature { name: "network" }`.
No rule means off. It is a bounded capability ([capabilities](../capabilities/README.md)): an
allow from the agent (the main agent for a conversation with none) or from the project allows
it and turns it on for their conversations, and a conversation then switches it off and back on
from the Tools dropdown in the composer. A conversation cannot switch it on where neither its
agent nor its project allows it; that answers 403 `capability_not_allowed`. The switch is
never asked about in a popup. With it off, a command that names hosts is not run and the
model is told why:

```rust
// server/src/chat.rs, scoped_tool_result
let on = matches!(self.agents.permission(&context, &switch,
    chrono::Utc::now().timestamp()).await.ok()?, PermissionDecision::Allow(_));
```

The switch is shown in the agent, project and conversation permission dialogs
([PermissionSettings.svelte](../../web/src/PermissionSettings.svelte)). In a conversation
it also offers "Use the default", which removes the conversation's own rule.

## Approving hosts

Hosts that still need an answer are decided in the command's own popup, as one decision
with the command and its working directory. "Approve Once" and "Approve for this session"
cover the listed hosts exactly. "Session, whole domains" stores, for the session, the
domain one level above each host ([parent_domain](../../server/src/permissions.rs)), or the
host itself where no wider rule is reasonable. A host whose command is already allowed is
asked about by itself.

A host inherits from `Target::Domain` rules for its own name and every domain above it; an
exact host rule, including a conversation block, takes precedence. A domain rule cannot be
a public-suffix shape such as `co.uk` or a shared hosting domain such as `github.io`. That
check is a short built-in list, not the full Public Suffix List; the popup and the settings
always show the exact domain. Lasting host and domain rules are added in the permission
dialogs like file rules.

## The proxy

[egress.rs](../../server/src/egress.rs) starts one proxy per command, on a free loopback
port, and stops it when the command ends. It accepts only `CONNECT` to port 443 of an
approved host, answers anything else with `403`, and records what it refused. Approved
names are resolved by the proxy, never by the command, and private, loopback, link-local,
carrier-grade NAT and unique-local addresses are dropped, so an approved name cannot lead to
this Mac, the VM host or the local network:

```rust
// server/src/egress.rs, system_resolver
.map(|found| found.filter(|address| !private(&address.ip())).collect())
```

The Seatbelt profile gains one rule, for that port only
([profile_with](../../server/src/seatbelt.rs)). The command receives `HTTPS_PROXY`,
`https_proxy`, `ALL_PROXY` and `all_proxy`; a tool that ignores them has no network. A
command with hosts may run for 120 seconds instead of 10. Refused destinations are named in
the result the model receives, so it can ask for a host instead of guessing.

In a VM the same code runs in that VM's worker, so commands there follow the same rule.
That needs a worker built after this change (`miniclaw update-vm`).

## What this does not protect against

A file the conversation may read can be sent to a host the user approved. That is why
hosts need approval and why network is off by default. Traffic inside the HTTPS connection
is not inspected.

## Verification

- `server/tests/seatbelt.rs`: `a_command_reaches_only_its_approved_hosts_and_only_through_the_proxy`
  (approved host reached, another host and another port refused with 403, a direct
  connection to the same server refused) and `a_command_without_hosts_has_no_proxy`.
- `server/src/egress.rs` tests: local and private addresses are refused.
- `server/tests/permissions.rs`: exact host rules, domain rules and their precedence, name
  checks, and the switch's default and precedence.
- [host-tools.test.mjs](../../tests/host-tools.test.mjs): off by default with a reason and
  no popup; one popup with the hosts; the command sees its proxy.
- [permissions.spec.ts](../../tests/browser/permissions.spec.ts): the popup lists hosts and
  sends the wider choice; the dialog switches network on, off and back to the default.

Observed on 2026-09-20 with `MINICLAW_REAL_NETWORK=1`: inside the sandbox, `curl` fetched
real package metadata from `registry.npmjs.org` through the proxy, `example.com` was refused
with 403, and a direct connection to the approved host failed. Not yet observed: a whole
`npm install`, and the feature inside a real VM.
