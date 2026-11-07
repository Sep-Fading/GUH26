// Checks the MCP server list: what is saved, what is refused, and that a secret never comes back.
import assert from 'node:assert/strict';
// Registers the list and import tests.
import { test } from 'node:test';
// Writes provider configuration files to import from.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
// Builds paths inside the temporary provider homes.
import { dirname, join } from 'node:path';
// Starts the backend against the fake Codex.
import { launchApp, json } from './support/harness.mjs';
// Supplies the signed-out account answer.
import { signedOut } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
const list = async app => (await json(await app.request('/api/mcp/servers'))).servers;
const SECRET = 'tok-7f3a-never-shown';
const windows = process.platform === 'win32';

/** Starts a backend whose secrets go to a private file instead of the real Keychain. */
async function appFor(t) {
  const app = await launchApp();
  t.after(() => app.close());
  return app;
}

/** A local command server as the page would send it. */
const local = (extra = {}) => ({ name: 'notes', kind: 'stdio', command: process.execPath, args: ['--stdio'], ...extra });
/** A remote server as the page would send it. */
const remote = (extra = {}) => ({ name: 'tracker', kind: 'http', url: 'https://mcp.example.com/mcp', ...extra });

test('a server is saved switched on nowhere, sandboxed, and survives a restart', async t => {
  const app = await appFor(t);
  const { server } = await json(await post(app, '/api/mcp/servers', local()), 201);
  assert.deepEqual([server.name, server.kind, server.command, server.args, server.unrestricted, server.revision],
    ['notes', 'stdio', process.execPath, ['--stdio'], false, 1]);
  await app.restart();
  assert.deepEqual((await list(app)).map(saved => saved.id), [server.id]);
  const edited = await json(await post(app, `/api/mcp/servers/${server.id}`, local({ unrestricted: true, revision: 1 })));
  assert.deepEqual([edited.server.unrestricted, edited.server.revision], [true, 2]);
  assert.equal((await post(app, `/api/mcp/servers/${server.id}`, local({ revision: 1 }))).status, 409, 'a stale edit is refused');
  await json(await post(app, `/api/mcp/servers/${server.id}/delete`, { revision: 2 }));
  assert.deepEqual(await list(app), []);
});

test('invalid servers are refused', async t => {
  const app = await appFor(t);
  for (const bad of [local({ name: '' }), local({ name: 'Has Space' }), local({ name: 'x'.repeat(41) }), local({ name: 'miniclaw' }),
    local({ command: '' }), local({ command: 'relative/path' }), local({ args: ['a\0b'] }), local({ url: 'https://x.example' }),
    local({ env: { 'BAD NAME': 'x' } }), local({ kind: 'sse' }), remote({ url: 'http://mcp.example.com/mcp' }),
    remote({ url: 'https://user:pw@mcp.example.com/' }), remote({ url: 'ftp://mcp.example.com' }), remote({ command: '/bin/sh' }),
    remote({ unrestricted: true }), remote({ token: 'a\nb' }), local({ enabled: true })]) {
    assert.equal((await post(app, '/api/mcp/servers', bad)).status, 400, JSON.stringify(bad));
  }
  await json(await post(app, '/api/mcp/servers', local()), 201);
  const taken = await json(await post(app, '/api/mcp/servers', local()), 409);
  assert.equal(taken.error.code, 'name_taken');
  // A server on this Mac may use plain HTTP; nothing else may.
  await json(await post(app, '/api/mcp/servers', remote({ name: 'dev', url: 'http://127.0.0.1:8931/mcp' })), 201);
});

test('a token and environment values are write-only', async t => {
  const app = await appFor(t);
  const { server } = await json(await post(app, '/api/mcp/servers', remote({ token: SECRET })), 201);
  assert.equal(server.hasToken, true);
  const { server: command } = await json(await post(app, '/api/mcp/servers', local({ env: { API_KEY: SECRET, REGION: 'eu' } })), 201);
  assert.deepEqual(command.env, ['API_KEY', 'REGION']);
  const everything = JSON.stringify([server, command, await list(app)]);
  assert.equal(everything.includes(SECRET), false, 'the secret never returns to the page');
  // The metadata database holds no secret either.
  for (const name of ['agents.sqlite3', 'agents.sqlite3-wal']) {
    const stored = await readFile(join(app.peer.dir, name)).catch(() => Buffer.alloc(0));
    assert.equal(stored.includes(SECRET), false, name);
  }
  // An edit that names no token keeps it; null removes it; a value kept by name stays.
  const kept = await json(await post(app, `/api/mcp/servers/${server.id}`, remote({ revision: 1 })));
  assert.equal(kept.server.hasToken, true);
  const cleared = await json(await post(app, `/api/mcp/servers/${server.id}`, remote({ revision: 2, token: null })));
  assert.equal(cleared.server.hasToken, false);
  const fewer = await json(await post(app, `/api/mcp/servers/${command.id}`, local({ revision: 1, env: { REGION: null } })));
  assert.deepEqual(fewer.server.env, ['REGION']);
});

test('servers are imported from Codex and Claude Code, without their secrets, switched on nowhere', async t => {
  const app = await appFor(t);
  await writeFile(join(app.peer.codexHome, 'config.toml'), [
    '[mcp_servers.context7]', 'command = "npx"', 'args = ["-y", "@upstash/context7-mcp"]', '',
    '[mcp_servers.context7.env]', `KEY = "${SECRET}"`, '',
    '[mcp_servers.figma]', 'url = "https://mcp.figma.example/mcp"', 'bearer_token_env_var = "FIGMA_TOKEN"', ''].join('\n'));
  const claudeHome = join(app.peer.dir, 'claude-home');
  await mkdir(claudeHome, { recursive: true });
  await writeFile(join(claudeHome, '.claude.json'), JSON.stringify({
    mcpServers: { hubspot: { type: 'http', url: 'https://mcp.hubspot.example/x', headers: { Authorization: `Bearer ${SECRET}` } } },
    projects: { '/tmp/demo': { mcpServers: { files: { type: 'stdio', command: '/usr/local/bin/files-mcp', args: [], env: { K: SECRET } } } } },
  }));
  const codex = await json(await app.request('/api/mcp/sources?provider=codex'));
  assert.deepEqual(codex.sources.map(source => [source.name, source.kind]), [['context7', 'stdio'], ['figma', 'http']]);
  const claude = await json(await app.request('/api/mcp/sources?provider=claude'));
  assert.deepEqual(claude.sources.map(source => source.name).sort(), ['files', 'hubspot']);
  assert.equal(JSON.stringify([codex, claude]).includes(SECRET), false, 'a listing shows names of secrets only');
  assert.deepEqual(codex.sources[0].env, ['KEY']);
  const imported = await post(app, '/api/mcp/import', { provider: 'codex', name: 'context7' });
  if (windows) {
    // Windows has `npx` only as npx.cmd, a batch file the backend never starts (cmd.exe would
    // parse its arguments again). The import saves the bare name, and the server runs with
    // MiniClaw's own Node and npm; the page is told so.
    const { server } = await json(imported, 201);
    assert.deepEqual([server.name, server.command, server.args, server.unrestricted], ['context7', 'npx', ['-y', '@upstash/context7-mcp'], false]);
    assert.match(server.runs, /MiniClaw's Node runs the command of @upstash\/context7-mcp/);
    assert.match(server.runs, /registry\.npmjs\.org/);
    assert.ok(server.needs.includes('KEY'));
    const listed = (await list(app)).find(saved => saved.id === server.id);
    assert.equal(listed.runs, server.runs);
  } else {
    const { server } = await json(imported, 201);
    // `npx` is found on the PATH at import time, so what is saved is an absolute path or the import is refused.
    assert.deepEqual([server.name, server.kind, server.args, server.env, server.unrestricted],
      ['context7', 'stdio', ['-y', '@upstash/context7-mcp'], [], false]);
    assert.match(server.command, /^\//);
    assert.ok(server.needs.includes('KEY'), 'it says which values the user still has to enter');
  }
  const { server: hub } = await json(await post(app, '/api/mcp/import', { provider: 'claude', name: 'hubspot' }), 201);
  assert.deepEqual([hub.url, hub.hasToken], ['https://mcp.hubspot.example/x', false]);
  assert.equal((await post(app, '/api/mcp/import', { provider: 'claude', name: 'hubspot' })).status, 409);
  assert.equal((await post(app, '/api/mcp/import', { provider: 'claude', name: 'absent' })).status, 404);
  // Importing reads; it never changes the provider's own file.
  assert.match(await readFile(join(app.peer.codexHome, 'config.toml'), 'utf8'), /context7/);
  if (windows) {
    // What Windows does import: a command named by the full path of a native .exe, kept as it is;
    // npm's launcher named by the path of npm.cmd, saved by name; any other batch file is refused,
    // and so is an npm command MiniClaw would not start. JSON strings are valid TOML strings.
    const toml = (name, command, args = []) => [`[mcp_servers.${name}]`, `command = ${JSON.stringify(command)}`, `args = ${JSON.stringify(args)}`, ''];
    await writeFile(join(app.peer.codexHome, 'config.toml'), [
      ...toml('native', process.execPath, ['--stdio']),
      ...toml('shim', join(dirname(process.execPath), 'npm.cmd'), ['exec', '--', 'server-pkg@1.2.3', 'a&b', '%PATH%', 'two words']),
      ...toml('batch', join(dirname(process.execPath), 'server.cmd')),
      ...toml('shell', 'npx', ['-c', 'echo hi'])].join('\n'));
    const { server } = await json(await post(app, '/api/mcp/import', { provider: 'codex', name: 'native' }), 201);
    assert.deepEqual([server.command, server.args, server.unrestricted, server.runs], [process.execPath, ['--stdio'], false, undefined]);
    const { server: shim } = await json(await post(app, '/api/mcp/import', { provider: 'codex', name: 'shim' }), 201);
    assert.deepEqual([shim.command, shim.args], ['npm', ['exec', '--', 'server-pkg@1.2.3', 'a&b', '%PATH%', 'two words']]);
    assert.match(shim.runs, /server-pkg@1\.2\.3 with a&b %PATH% two words/);
    for (const name of ['batch', 'shell']) {
      const { error } = await json(await post(app, '/api/mcp/import', { provider: 'codex', name }), 400);
      assert.equal(error.code, 'invalid_request', name);
    }
  }
});

test('the list needs the signed-in account', async t => {
  const app = await appFor(t);
  await app.set('account/read', { result: signedOut });
  await app.request('/api/account');
  assert.equal((await app.request('/api/mcp/servers')).status, 401);
  assert.equal((await post(app, '/api/mcp/servers', local())).status, 401);
});
