// Checks future environment selection and persistence.
import assert from 'node:assert/strict';
// Runs isolated acceptance cases before implementation.
import { test } from 'node:test';
// Starts disposable backends with synthetic Codex.
import { launchApp, json } from './support/harness.mjs';
// Uses an existing fixture chat that predates machine assignment.
import { THREAD, CLIENT } from './support/fixtures.mjs';

// Sends a same-origin mutation without touching the running app.
const post = (app, path, body) => app.request(path, { method: 'POST', body });

for (const environment of [
  { kind: 'host' },
  { kind: 'vm', dedicated: false },
]) {
  test(`project keeps ${JSON.stringify(environment)}`, async t => {
    const app = await launchApp();
    t.after(() => app.close());
    const { project } = await json(await post(app, '/api/projects', {
      name: 'Environment fixture', environment,
    }), 201);
    assert.deepEqual(project.environment, environment);
    await app.restart();
    const { projects } = await json(await app.request('/api/projects'));
    assert.deepEqual(projects.find(p => p.id === project.id).environment, environment);
    assert.equal((await app.inspect()).requests.filter(r =>
      r.method === 'thread/start').length, 0, 'saving preferences must not start work');
  });
}

test('dedicated project stays unsaved without a ready VM', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const response = await post(app, '/api/projects', {
    name: 'Dedicated fixture', environment: { kind: 'vm', dedicated: true },
  });
  assert.equal(response.status, 503);
  assert.match((await response.json()).error.message, /VM|virtual machine/i);
  assert.deepEqual((await json(await app.request('/api/projects'))).projects, []);
  assert.equal((await app.inspect()).requests.filter(r =>
    r.method === 'thread/start').length, 0);
});

test('project environment is immutable', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { project } = await json(await post(app, '/api/projects', {
    name: 'Host fixture', environment: { kind: 'host' },
  }), 201);
  const { id, ...definition } = project;
  await json(await post(app, `/api/projects/${id}`, {
    ...definition, environment: { kind: 'vm', dedicated: false },
  }), 409);
  const { projects } = await json(await app.request('/api/projects'));
  assert.deepEqual(projects.find(p => p.id === id), project);
});

test('project creation requires an environment', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/projects', { name: 'Missing choice' }), 400);
  assert.deepEqual((await json(await app.request('/api/projects'))).projects, []);
});

test('standalone creation requires an environment', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await post(app, '/api/threads', {}), 400);
  assert.equal((await app.inspect()).requests.filter(r =>
    r.method === 'thread/start').length, 0, 'missing choice must not launch Codex work');
});

test('explicit host choice is returned', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const environment = { kind: 'host' };
  const { thread } = await json(await post(app, '/api/threads', { environment }), 201);
  assert.deepEqual(thread.environment, environment);
  await app.restart();
  const snapshot = await json(await app.request(`/api/threads/${thread.id}`));
  assert.deepEqual(snapshot.thread.environment, environment);
  await json(await post(app, `/api/threads/${thread.id}/messages`, {
    text: 'Continue on Host', clientMessageId: CLIENT,
  }), 202);
  assert.equal((await app.inspect()).requests.filter(r =>
    r.method === 'turn/start').length, 1);
});

test('old chat cannot send until its machine is chosen', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await app.request(`/api/threads/${THREAD}`));
  const response = await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Do not guess the machine', clientMessageId: CLIENT,
  });
  assert.equal(response.status, 409);
  assert.match((await response.json()).error.message, /machine|environment/i);
  assert.equal((await app.inspect()).requests.filter(r =>
    r.method === 'turn/start').length, 0);
});

test('old chat gets one permanent verified assignment', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  await json(await app.request(`/api/threads/${THREAD}`));
  const path = `/api/threads/${THREAD}/environment`;
  const wrong = await post(app, path, { kind: 'vm' });
  assert.equal(wrong.status, 503);
  const assigned = await json(await post(app, path, { kind: 'host' }));
  assert.deepEqual(assigned.environment, { kind: 'host' });
  await json(await post(app, path, { kind: 'vm' }), 409);
  await app.restart();
  const snapshot = await json(await app.request(`/api/threads/${THREAD}`));
  assert.deepEqual(snapshot.thread.environment, { kind: 'host' });
  await json(await post(app, `/api/threads/${THREAD}/messages`, {
    text: 'Continue on assigned Mac', clientMessageId: CLIENT,
  }), 202);
  assert.equal((await app.inspect()).requests.filter(r =>
    r.method === 'turn/start').length, 1);
});

test('project chat can choose Host under a VM project', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { project } = await json(await post(app, '/api/projects', {
    name: 'VM project', environment: { kind: 'vm', dedicated: false },
  }), 201);
  const { thread } = await json(await post(app, '/api/threads', {
    projectId: project.id, environment: { kind: 'host' },
  }), 201);
  assert.equal(thread.projectId, project.id);
  assert.deepEqual(thread.environment, { kind: 'host' });
});

test('VM choice cannot use a Host worker', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  const { project } = await json(await post(app, '/api/projects', {
    name: 'Host project', environment: { kind: 'host' },
  }), 201);
  const response = await post(app, '/api/threads', {
    projectId: project.id, environment: { kind: 'vm' },
  });
  assert.equal(response.status, 503);
  assert.match((await response.json()).error.message, /VM|virtual machine/i);
  assert.equal((await app.inspect()).requests.filter(r =>
    r.method === 'thread/start').length, 0);
});

test('invalid environment cannot create work', async t => {
  const app = await launchApp();
  t.after(() => app.close());
  for (const environment of [{ kind: 'remote' }, { kind: 'host', vmId: 'master' }]) {
    await json(await post(app, '/api/threads', { environment }), 400);
  }
  assert.equal((await app.inspect()).requests.filter(r =>
    r.method === 'thread/start').length, 0);
});
