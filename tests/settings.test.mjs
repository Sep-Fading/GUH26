// Checks the appearance settings MiniClaw keeps on the Mac.
import assert from 'node:assert/strict';
// Runs each settings case against a disposable backend.
import { test } from 'node:test';
// Reads the stored file's mode.
import { stat } from 'node:fs/promises';
import { join } from 'node:path';
// Starts the backend and reads JSON replies.
import { launchApp, json } from './support/harness.mjs';
// Windows has no mode bits; private state is a protected access list for the current user.
import { assertWindowsPrivate } from './support/windows-acl.mjs';

const post = (app, path, body) => app.request(path, { method: 'POST', body });
const defaults = { theme: 'system', textSize: 'default', accent: 'blue', compact: false };

test('appearance starts from the defaults, is saved on the Mac and survives a restart', async t => {
  const app = await launchApp(); t.after(() => app.close());
  assert.deepEqual(await json(await app.request('/api/settings/appearance')), defaults);
  const chosen = { theme: 'dark', textSize: 'large', accent: 'green', compact: true };
  assert.deepEqual(await json(await post(app, '/api/settings/appearance', chosen)), chosen);
  if (process.platform === 'win32') await assertWindowsPrivate(join(app.peer.dir, 'appearance.json'));
  else assert.equal((await stat(join(app.peer.dir, 'appearance.json'))).mode & 0o777, 0o600);
  await app.restart();
  assert.deepEqual(await json(await app.request('/api/settings/appearance')), chosen);
});

test('appearance accepts only known choices and needs no sign-in', async t => {
  const app = await launchApp(); t.after(() => app.close());
  for (const bad of [{ ...defaults, theme: 'neon' }, { ...defaults, textSize: 'huge' },
    { ...defaults, accent: 'url(evil)' }, { ...defaults, compact: 'yes' }, { theme: 'dark' },
    { ...defaults, extra: 1 }]) {
    assert.equal((await post(app, '/api/settings/appearance', bad)).status, 400, JSON.stringify(bad));
  }
  assert.deepEqual(await json(await app.request('/api/settings/appearance')), defaults);
  // The page must be able to theme itself before anyone has signed in.
  const signedOut = await launchApp({ 'account/read': { result: { account: null } } });
  t.after(() => signedOut.close());
  assert.equal((await signedOut.request('/api/settings/appearance')).status, 200);
});
