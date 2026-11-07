// Checks provider discovery without touching real accounts.
import assert from 'node:assert/strict';
// Runs each discovery case in isolation.
import { test } from 'node:test';
// Builds real Windows installation layouts in a disposable folder.
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Loads the discovery under test.
import { detectProviders } from '../scripts/setup/providers.mjs';

// The macOS cases name app bundles under /Applications, which Windows discovery never looks in.
const macOnly = { skip: process.platform === 'win32' && 'macOS app-bundle locations; Windows discovery is tested below with real files' };

test('installed providers are reported with their sign-in state and no identity', async () => {
  const calls = [];
  const found = await detectProviders({
    find: async name => `/test/bin/${name}`,
    probe: async (path, args) => { calls.push([path, ...args]);
      return path.endsWith('claude') ? { code: 0, stdout: '{"loggedIn":true,"email":"me@example.test"}' }
        : { code: 1, stdout: 'Not logged in' }; },
  });
  assert.deepEqual(found, [
    { id: 'codex', name: 'Codex (ChatGPT)', path: '/test/bin/codex', signedIn: false },
    { id: 'claude', name: 'Claude Code', path: '/test/bin/claude', signedIn: true },
  ]);
  assert.deepEqual(calls, [['/test/bin/codex', 'login', 'status'], ['/test/bin/claude', 'auth', 'status', '--json']]);
  assert.equal(JSON.stringify(found).includes('example.test'), false);
});

test('a missing or failing CLI is left out or shown as signed out', async () => {
  const found = await detectProviders({
    find: async name => name === 'claude' ? '/test/claude' : null,
    probe: async () => { throw new Error('spawn failed'); },
  });
  assert.deepEqual(found, [{ id: 'claude', name: 'Claude Code', path: '/test/claude', signedIn: false }]);
});

test('Codex is looked for in both desktop apps', macOnly, async () => {
  const seen = {};
  await detectProviders({ find: async (name, candidates) => { seen[name] = candidates; return null; },
    probe: async () => ({ code: 1, stdout: '' }) });
  assert.ok(seen.codex.includes('/Applications/Codex.app/Contents/Resources/codex'));
  assert.ok(seen.codex.includes('/Applications/ChatGPT.app/Contents/Resources/codex'));
  assert.ok(seen.claude.some(path => path.endsWith('.local/bin/claude')));
});

for (const app of ['Codex', 'ChatGPT']) {
  test(`${app} prefers the packaged launcher over the legacy binary`, macOnly, async () => {
    const root = `/Applications/${app}.app/Contents/Resources`;
    const available = new Set([`${root}/codex-cli/bin/codex`, `${root}/codex`]);
    const found = await detectProviders({
      find: async (name, candidates) => name === 'codex'
        ? candidates.find(path => available.has(path)) : null,
      probe: async () => ({ code: 0, stdout: '' }),
    });
    assert.equal(found[0]?.path, `${root}/codex-cli/bin/codex`);
  });
}

test('legacy desktop layouts remain discoverable', macOnly, async () => {
  const path = '/Applications/ChatGPT.app/Contents/Resources/codex';
  const found = await detectProviders({
    find: async (name, candidates) => name === 'codex' && candidates.includes(path) ? path : null,
    probe: async () => ({ code: 0, stdout: '' }),
  });
  assert.equal(found[0]?.path, path);
});

// Real files and the real executable lookup: the desktop Codex wins over npm's native payload,
// Claude's own installer wins over npm, and npm's .cmd launchers on PATH are never chosen.
test('Windows finds the desktop Codex, then npm native builds, never a .cmd launcher', {
  skip: process.platform !== 'win32' && 'needs Windows file lookup (.exe suffixes, ; separated Path)',
}, async t => {
  const root = await mkdtemp(join(tmpdir(), 'mc-providers-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const home = join(root, 'home'), local = join(root, 'Local'), roaming = join(root, 'Roaming');
  const shims = join(roaming, 'npm');
  const desktop = join(local, 'Programs', 'OpenAI', 'Codex', 'bin', 'codex.exe');
  const npmCodex = join(shims, 'node_modules', '@openai', 'codex-win32-arm64', 'vendor',
    'aarch64-pc-windows-msvc', 'bin', 'codex.exe');
  const ownClaude = join(home, '.local', 'bin', 'claude.exe');
  const npmClaude = join(shims, 'node_modules', '@anthropic-ai', 'claude-code', 'node_modules',
    '@anthropic-ai', 'claude-code-win32-arm64', 'claude.exe');
  for (const path of [desktop, npmCodex, ownClaude, npmClaude]) {
    await mkdir(join(path, '..'), { recursive: true });
    await writeFile(path, 'MZ');
  }
  for (const name of ['codex.cmd', 'claude.cmd']) await writeFile(join(shims, name), '@echo off');
  const probed = [];
  const detect = () => detectProviders({ arch: 'arm64', home,
    env: { APPDATA: roaming, LOCALAPPDATA: local, Path: shims },
    probe: async (path, args) => { probed.push(path); return { code: 0, stdout: '{"loggedIn":true}' }; } });
  assert.deepEqual((await detect()).map(({ id, path }) => [id, path]), [['codex', desktop], ['claude', ownClaude]]);
  await rm(desktop); await rm(ownClaude);
  assert.deepEqual((await detect()).map(({ id, path }) => [id, path]), [['codex', npmCodex], ['claude', npmClaude]]);
  await rm(npmCodex); await rm(npmClaude);
  assert.deepEqual(await detect(), []);
  assert.equal(probed.some(path => /\.cmd$/i.test(path)), false);
});
