// Records what stops provider-native tools from sharing MiniClaw's Seatbelt profile.
import assert from 'node:assert/strict';
// Runs only when explicitly selected.
import { test } from 'node:test';
// Runs sandboxed commands and reads their output.
import { spawnSync } from 'node:child_process';
// Finds the Codex executable the app uses.
import { existsSync } from 'node:fs';

const enabled = process.env.MINICLAW_NATIVE_PROBE === '1';
const codex = ['/Applications/Codex.app/Contents/Resources/codex',
  '/Applications/ChatGPT.app/Contents/Resources/codex'].find(existsSync);
const run = (binary, args) => spawnSync(binary, args, { encoding: 'utf8', timeout: 20_000 });
const nested = outer => run('/usr/bin/sandbox-exec', ['-p', outer,
  '/usr/bin/sandbox-exec', '-p', '(version 1)(allow default)', '/bin/echo', 'inner-started']);

test('a restrictive profile cannot contain a second Seatbelt profile', { skip: !enabled }, () => {
  assert.match(nested('(version 1)(allow default)').stdout, /inner-started/);
  for (const outer of ['(version 1)(allow default)(deny network*)',
    '(version 1)(allow default)(deny file-read* (subpath "/private/var/empty"))']) {
    const result = nested(outer);
    assert.doesNotMatch(result.stdout, /inner-started/);
    assert.match(result.stderr, /sandbox_apply: Operation not permitted/);
  }
});

test('the Codex sandbox blocks writes and network but reads everything', { skip: !enabled || !codex }, () => {
  const inside = script => run(codex, ['sandbox', '--', '/bin/zsh', '-c', script]);
  // Only a byte count leaves the sandbox; the file's content is never printed.
  assert.ok(Number(inside('wc -c < ~/.codex/auth.json').stdout.trim()) > 0,
    "Codex's own sign-in file is readable by commands it runs");
  assert.match(inside('echo x > ~/mc-native-probe-write; echo exit=$?').stdout, /exit=1/);
  assert.doesNotMatch(inside('curl -sS -m 5 -o /dev/null -w "%{http_code}" https://example.com').stdout, /200/);
});
