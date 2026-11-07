// Checks the Files and Changes routes on a real git repository in a disposable folder.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { link, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { launchApp, json } from './support/harness.mjs';
import { DISCARD_UNAVAILABLE, closedRefusal, sandboxReport } from './support/sandbox.mjs';
import { ownAsUser } from './support/windows-links.mjs';
import { THREAD } from './support/fixtures.mjs';

const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
// While Windows cannot enforce the sandbox that runs Git, the backend runs no Git at all: Changes
// reports the conversation view of a repository and discard answers `discard_unavailable`. Only
// that report selects the expectations, so these tests check the Git view once the gate opens.
// The repository fixture shows the conversation view exactly when the report says Git is unavailable.
async function gitGateClosed(app, changes) {
  const closed = process.platform === 'win32' && changes.source === 'conversation';
  if (process.platform === 'win32' && closed && (await sandboxReport(app)).git.available) {
    // Git ran and refused the repository. Under an elevated token new folders belong to
    // Administrators, whom the bundled Git does not trust; that is this run's token, not the product.
    const owner = execFileSync('powershell.exe', ['-NoProfile', '-Command', `(Get-Acl -LiteralPath '${changes.root.replaceAll("'", "''")}').Owner`], { encoding: 'utf8' }).trim();
    assert.fail(owner === 'BUILTIN\\Administrators'
      ? 'The fixture repository is owned by Administrators (elevated token), so Git refuses it: run this suite with the ordinary token'
      : `Changes shows the conversation view although Git is available (repository owner ${owner})`);
  }
  if (process.platform === 'win32') assert.equal(closed, !(await sandboxReport(app)).git.available, 'Changes follows the sandbox report');
  return closed;
}
const git = (cwd, ...args) => execFileSync('git', ['-c', 'user.name=T', '-c', 'user.email=t@example.test', ...args], { cwd, stdio: 'pipe' });

// A project whose workspace is a small repository with one commit, and a conversation in it.
async function fixture(t, { repository = true } = {}) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'mc-workspace-')));
  const site = join(root, 'site');
  await mkdir(join(site, 'src'), { recursive: true });
  await writeFile(join(site, 'README.md'), '# Site\n\nFirst line.\n');
  await writeFile(join(site, 'src/app.js'), 'const a = 1;\nconsole.log(a);\n');
  await writeFile(join(root, 'outside.txt'), 'OUTSIDE-SECRET');
  if (repository) { git(site, 'init', '-q'); git(site, 'add', '.'); git(site, 'commit', '-qm', 'first'); }
  ownAsUser(root);
  const app = await launchApp({}, { git: true, env: { MINICLAW_TRASH_DIR: join(root, 'Trash') } });
  t.after(async () => { await app.close(); await rm(root, { recursive: true, force: true }); });
  const { project } = await json(await post(app, '/api/projects', { name: 'Site', environment: { kind: 'host' }, workspace: site }), 201);
  // Starting the conversation surveys the folder with the sandboxed Git. The first run of a freshly
  // installed Git took about 7 s on the Windows test guest (measured 2026-10-05), later ones 0.25 s.
  await json(await app.request('/api/threads', { method: 'POST', body: { environment: { kind: 'host' }, projectId: project.id }, signal: AbortSignal.timeout(60000) }), 201);
  const get = async (path, status = 200) => json(await app.request(`/api/threads/${THREAD}/workspace/${path}`), status);
  return { app, root, site, get };
}

test('Files lists the working folder and reads a file, and never leaves the folder', async t => {
  const { site, get } = await fixture(t);
  // Windows lets an ordinary token create a file link only in Developer Mode; without one the
  // link case is left out here (the Rust file-policy tests plant links with their own handles).
  const linked = await symlink(join(site, '../outside.txt'), join(site, 'leak.txt')).then(() => true,
    error => { if (process.platform === 'win32' && error.code === 'EPERM') return false; throw error; });
  const top = await get('files?path=');
  assert.equal(top.root, site);
  assert.deepEqual(top.entries.map(entry => [entry.name, entry.kind]), [['src', 'folder'], ['README.md', 'file']],
    'folders first; .git and links are left out');
  assert.deepEqual((await get('files?path=src')).entries.map(entry => entry.name), ['app.js']);
  const file = await get(`file?path=${encodeURIComponent('src/app.js')}`);
  assert.equal(file.text, 'const a = 1;\nconsole.log(a);\n');
  for (const bad of ['../outside.txt', '/etc/hosts', 'src/../../outside.txt', ...(linked ? ['leak.txt'] : []), '.git/config']) {
    const response = await fetchStatus(get, `file?path=${encodeURIComponent(bad)}`);
    assert.ok([400, 404].includes(response), `${bad} -> ${response}`);
  }
});
const fetchStatus = async (get, path) => { try { await get(path); return 200; } catch (error) { return Number(/actual: (\d+)/.exec(String(error.message) + JSON.stringify(error))?.[1] ?? error.actual ?? 0); } };

test('Changes shows the git working tree with counts and a diff per file', async t => {
  const { app, site, get } = await fixture(t);
  await writeFile(join(site, 'src/app.js'), 'const a = 2;\nconsole.log(a);\nconsole.log("done");\n');
  await writeFile(join(site, 'new.txt'), 'one\ntwo\n');
  await rm(join(site, 'README.md'));
  const changes = await get('changes');
  if (await gitGateClosed(app, changes)) {
    assert.deepEqual(changes, { source: 'conversation', root: site, files: [] });
    assert.deepEqual(await get(`diff?path=${encodeURIComponent('src/app.js')}`), { path: 'src/app.js', diff: '' });
    return;
  }
  assert.equal(changes.source, 'git');
  assert.deepEqual(changes.files.map(file => [file.path, file.status, file.added, file.removed]),
    [['README.md', 'deleted', 0, 3], ['new.txt', 'new', 2, 0], ['src/app.js', 'modified', 2, 1]]);
  const diff = (await get(`diff?path=${encodeURIComponent('src/app.js')}`)).diff;
  assert.match(diff, /^-const a = 1;$/m);
  assert.match(diff, /^\+const a = 2;$/m);
  assert.match((await get('diff?path=new.txt')).diff, /^\+one$/m);
});

test('Discard restores a tracked file and moves a new file to the Trash, after nothing else', async t => {
  const { app, root, site, get } = await fixture(t);
  await writeFile(join(site, 'src/app.js'), 'changed\n');
  await writeFile(join(site, 'new.txt'), 'keep me recoverable\n');
  const discard = path => post(app, `/api/threads/${THREAD}/workspace/discard`, { path });
  assert.ok([400, 404].includes((await discard('../outside.txt')).status));
  if (await gitGateClosed(app, await get('changes'))) {
    for (const path of ['src/app.js', 'new.txt']) {
      const refused = await json(await discard(path), 409);
      assert.deepEqual([refused.error.code, refused.error.message], ['discard_unavailable', await closedRefusal(app, DISCARD_UNAVAILABLE, 'git')]);
    }
    assert.equal(await readFile(join(site, 'src/app.js'), 'utf8'), 'changed\n', 'nothing was restored');
    assert.equal(await readFile(join(site, 'new.txt'), 'utf8'), 'keep me recoverable\n', 'nothing was moved');
    assert.equal(existsSync(join(root, 'Trash')), false);
    return;
  }
  await json(await discard('src/app.js'));
  assert.equal(await readFile(join(site, 'src/app.js'), 'utf8'), 'const a = 1;\nconsole.log(a);\n');
  const recovered = await json(await discard('new.txt'));
  assert.equal(typeof recovered.recoveryPath, 'string');
  assert.equal(existsSync(join(site, 'new.txt')), false);
  assert.equal(await readFile(recovered.recoveryPath, 'utf8'), 'keep me recoverable\n');
  // On macOS the discarded file lands in the Trash under its own name.
  if (process.platform !== 'win32') assert.equal(await readFile(join(root, 'Trash/new.txt'), 'utf8'), 'keep me recoverable\n');
  assert.equal(await readFile(join(root, 'outside.txt'), 'utf8'), 'OUTSIDE-SECRET');
});

test('a planted git configuration cannot make MiniClaw run a command outside the sandbox', async t => {
  const { root, site, get } = await fixture(t);
  const proof = join(root, 'escaped');
  // fsmonitor and an external diff are the usual ways a repository makes git run a program.
  await writeFile(join(site, 'hook.sh'), `#!/bin/sh\ntouch ${proof}\n`, { mode: 0o755 });
  git(site, 'config', 'core.fsmonitor', join(site, 'hook.sh'));
  git(site, 'config', 'diff.external', join(site, 'hook.sh'));
  await writeFile(join(site, 'src/app.js'), 'const a = 3;\n');
  await get('changes');
  await get(`diff?path=${encodeURIComponent('src/app.js')}`);
  assert.equal(existsSync(proof), false, 'no planted program ran with access outside the folder');
});

test('without a repository Changes falls back to what the agent changed, and says so', async t => {
  const { get } = await fixture(t, { repository: false });
  const changes = await get('changes');
  assert.equal(changes.source, 'conversation');
  assert.deepEqual(changes.files, []);
});

// A sandboxed command could change a file through any of its names, so the Windows sandbox refuses a
// folder that holds a file with a second name outside it (a package store does this). Git then
// cannot run, and discard must fail rather than treat a tracked file as new and move it away.
test('Discard fails, and moves nothing, when the sandbox refuses the folder before Git runs',
  { skip: process.platform !== 'win32' && 'only the Windows sandbox refuses a folder at grant time' }, async t => {
  const { app, root, site, get } = await fixture(t);
  if (!(await sandboxReport(app)).git.available) return;
  await link(join(root, 'outside.txt'), join(site, 'linked.txt'));
  await writeFile(join(site, 'README.md'), 'changed\n');
  assert.equal((await get('changes')).source, 'conversation', 'Git could not run, so Changes falls back');
  const refused = await json(await post(app, `/api/threads/${THREAD}/workspace/discard`, { path: 'README.md' }), 409);
  assert.equal(refused.error.code, 'discard_failed');
  assert.equal(await readFile(join(site, 'README.md'), 'utf8'), 'changed\n', 'the tracked file stays where it is');
  assert.equal(existsSync(join(root, 'Trash')), false, 'nothing was moved away');
});

test('Git treats quoted filenames and pathspec syntax literally', async t => {
  const { app, site, get } = await fixture(t);
  const name = "O'Brien ’ $value.txt";
  await writeFile(join(site, name), 'original\n');
  git(site, 'add', '--', name); git(site, 'commit', '-qm', 'quoted fixture');
  await writeFile(join(site, name), 'changed\n');
  const diff = await get(`diff?path=${encodeURIComponent(name)}`);
  if (await gitGateClosed(app, await get('changes'))) return assert.deepEqual(diff, { path: name, diff: '' });
  assert.match(diff.diff, /-original/);
  assert.match(diff.diff, /\+changed/);
});

test('a file whose name holds a tab can be viewed and diffed', { skip: process.platform === 'win32' && 'Windows file names cannot hold a tab' }, async t => {
  const { site, get } = await fixture(t);
  const name = 'notes\tdraft.txt';
  await writeFile(join(site, name), 'original\n');
  git(site, 'add', '--', name); git(site, 'commit', '-qm', 'tab fixture');
  await writeFile(join(site, name), 'changed\n');
  assert.equal((await get(`file?path=${encodeURIComponent(name)}`)).text, 'changed\n');
  const diff = await get(`diff?path=${encodeURIComponent(name)}`);
  assert.match(diff.diff, /-original/);
  assert.match(diff.diff, /\+changed/);
});
