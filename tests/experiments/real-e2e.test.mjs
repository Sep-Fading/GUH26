// End-to-end check of MiniClaw with the real Codex and the real Claude CLI: real sign-ins, real
// model calls, a real browser. Nothing here is faked except the user, who is this script.
//
//   MINICLAW_NATIVE_MODEL_PROBE=1 node --test tests/experiments/real-e2e.test.mjs
//
// It uses small models only (never Astra) and deletes every conversation it made.
import assert from 'node:assert/strict';
import { test, before, after } from 'node:test';
// Builds the folder the agents may work in.
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
// Drives the real page.
import { chromium } from '@playwright/test';
// Starts a disposable backend on the installed providers.
import { launchApp, json, eventually } from '../support/harness.mjs';

const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const PROVIDERS = {
  codex: { model: 'gpt-5.6-luna', effort: 'low', label: 'Codex' },
  claude: { model: 'claude-haiku-4-5-20251001', effort: null, label: 'Claude' },
};
const report = [];
const note = (name, detail) => { report.push([name, detail]); console.log(`RESULT ${name}: ${detail}`); };

let app, root, work;
const post = (path, body = {}) => app.request(path, { method: 'POST', body });
const state = async thread => json(await app.request(`/api/threads/${thread}`));

// Saves an active agent on one provider; `grant` lets it read and write the work folder.
async function agent(provider, name, instructions, extra = {}, grant = false) {
  const { model, effort } = PROVIDERS[provider];
  const saved = (await json(await post('/api/agents', { name, instructions, active: true, model, effort, ...extra }), 201)).agent;
  if (grant) for (const action of ['read', 'write']) {
    await json(await post(`/api/permissions/agent/${saved.id}`,
      { target: { target_type: 'folder', path: work, action }, effect: 'allow' }));
  }
  return saved;
}
const chat = async (provider, agentId) => (await json(await post('/api/threads',
  { environment: { kind: 'host' }, agentId, model: PROVIDERS[provider].model, effort: PROVIDERS[provider].effort ?? undefined }), 201)).thread;

// Sends a message and plays the user until the turn ends: answers approvals and delegation
// requests as told, and returns the final snapshot.
async function converse(thread, provider, text, { approve = 'approve_once', delegate = 'wait', timeout = 240_000 } = {}) {
  const before = (await state(thread)).messages.length;
  await json(await post(`/api/threads/${thread}/messages`, { text, clientMessageId: crypto.randomUUID(),
    model: PROVIDERS[provider].model, effort: PROVIDERS[provider].effort ?? undefined }), 202);
  const answered = new Set();
  const seen = { approvals: [], delegations: [] };
  const snapshot = await eventually(async () => {
    const value = await state(thread);
    if (value.scopedApproval && !answered.has(value.scopedApproval.id)) {
      answered.add(value.scopedApproval.id);
      seen.approvals.push(value.scopedApproval);
      const target = value.scopedApproval.thread ?? thread;
      await post(`/api/threads/${target}/scoped-approvals/${value.scopedApproval.id}`, { action: approve });
    }
    // A delegated agent's request waits in the parent's per-run queue (since 2026-09-27) and is
    // answered at the run's own thread.
    for (const item of value.delegatedRequests ?? []) {
      if (item.kind !== 'scoped' || answered.has(item.request.id)) continue;
      answered.add(item.request.id);
      seen.approvals.push({ ...item.request, agent: item.agent, thread: item.thread });
      await post(`/api/threads/${item.thread}/scoped-approvals/${item.request.id}`, { action: approve });
    }
    if (value.delegationPrompt && !answered.has(value.delegationPrompt.id)) {
      answered.add(value.delegationPrompt.id);
      seen.delegations.push(value.delegationPrompt);
      await post(`/api/threads/${thread}/delegations/${value.delegationPrompt.id}`, { mode: delegate });
    }
    return value;
  }, value => !value.activeTurn && !value.pending && value.messages.length > before
    && value.messages.at(-1)?.role === 'assistant', `${provider} turn to finish`, timeout);
  return { ...snapshot, seen, answer: snapshot.messages.at(-1).text };
}

before(async () => {
  if (!enabled) return;
  root = await realpath(await mkdtemp(join(tmpdir(), 'mc-real-e2e-')));
  work = join(root, 'work');
  await mkdir(work);
  app = await launchApp({}, { real: true, env: { CLAUDE_BIN: process.env.CLAUDE_BIN ?? join(homedir(), '.local/bin/claude'),
    // A "minute" of approval wait lasts five seconds here, so termination can be observed.
    MINICLAW_TEST_MINUTE_MS: '5000' } });
  await eventually(async () => (await app.request('/api/account')).json(),
    value => value.providers?.every(provider => provider.status === 'signed_in'), 'both providers signed in', 30_000);
});

after(async () => {
  if (!enabled || !app) return;
  // Leave nothing behind in the real Codex and Claude histories.
  const { threads } = await json(await app.request('/api/threads'));
  let removed = 0;
  for (const thread of threads) if ((await post(`/api/threads/${thread.id}/delete`)).ok) removed += 1;
  console.log(`CLEANUP deleted ${removed} of ${threads.length} conversations`);
  console.log('\nSUMMARY\n' + report.map(([name, detail]) => `- ${name}: ${detail}`).join('\n'));
  await app.close();
  await rm(root, { recursive: true, force: true });
});

for (const provider of ['codex', 'claude']) {
  test(`${provider}: initial tool choices apply to the first prompt`, { skip: !enabled, timeout: 300_000 }, async () => {
    const builder = await agent(provider, `Initial-${provider}`, 'Follow the user request exactly.', {}, true);
    const { model, effort } = PROVIDERS[provider];
    const { thread } = await json(await post('/api/threads', { environment: { kind: 'host' },
      agentId: builder.id, model, effort: effort ?? undefined,
      capabilities: { full_access: true, web_search: false, questions: false } }), 201);
    const target = join(work, `initial-${provider}.txt`);
    const done = await converse(thread.id, provider,
      `Use miniclaw_scoped_native to write the exact text INITIAL-TOOLS-OK to ${target}. Then reply Done.`, { approve: 'decline' });
    assert.equal(await readFile(target, 'utf8'), 'INITIAL-TOOLS-OK');
    assert.equal(done.seen.approvals.length, 0, 'full access was active on the very first tool call');
    const { capabilities } = await json(await app.request(`/api/capabilities?thread=${thread.id}`));
    assert.equal(capabilities.find(item => item.key === 'web_search').on, false);
    note(`${provider} initial tools`, 'first-prompt write succeeded with the chosen switches and no approval popup');
  });

  test(`${provider}: a plain chat streams an answer and remembers the conversation`, { skip: !enabled, timeout: 300_000 }, async () => {
    const thread = (await json(await post('/api/threads', { environment: { kind: 'host' },
      model: PROVIDERS[provider].model, effort: PROVIDERS[provider].effort ?? undefined }), 201)).thread;
    const first = await converse(thread.id, provider, 'My favourite number is 7431. Reply with just OK.');
    const second = await converse(thread.id, provider, 'What is my favourite number? Reply with the number only.');
    assert.match(second.answer, /7431/);
    assert.ok(second.messages.every(message => message.role === 'user' || message.turnId), 'assistant messages name their turn');
    note(`${provider} plain chat`, `remembered across turns; first reply "${first.answer.slice(0, 40)}"`);
  });

  test(`${provider}: an agent edits a granted file and runs a command; both appear as activity with a diff`,
    { skip: !enabled, timeout: 400_000 }, async () => {
      const file = join(work, `notes-${provider}.txt`);
      await writeFile(file, 'Teh cat sat on the mat.\nSecond line.\n');
      const editor = await agent(provider, `Editor ${provider}`, 'You edit files with the miniclaw tool. Be brief.', {}, true);
      const thread = await chat(provider, editor.id);
      const done = await converse(thread.id, provider,
        `Use your miniclaw tool. First call replace_in_file on ${file} to replace "Teh" with "The". `
        // `wc` is on the harmless list (server/src/harmless.rs) and runs unasked; `sed` asks.
        + `Then call run_shell with the command "sed -n '$=' notes-${provider}.txt" and cwd ${work}. Then tell me the line count.`);
      assert.equal(await readFile(file, 'utf8'), 'The cat sat on the mat.\nSecond line.\n', 'the file really changed');
      const edit = done.activities.find(entry => entry.kind === 'edit');
      const command = done.activities.find(entry => entry.kind === 'command');
      assert.ok(edit, `an edit was recorded: ${JSON.stringify(done.activities.map(a => a.kind))}`);
      assert.match(edit.diff, /^-Teh cat/m);
      assert.match(edit.diff, /^\+The cat/m);
      assert.ok(command, `a command was recorded: ${JSON.stringify(done.activities)} / said: ${done.answer}`);
      assert.equal(command.exitCode, 0);
      assert.match(command.output, /2/);
      assert.equal(done.seen.approvals.length >= 1, true, 'the command asked for approval');
      assert.match(done.answer, /2/);
      note(`${provider} tool + activity`, `edit with diff, command exit 0, ${done.seen.approvals.length} approval(s); said "${done.answer.slice(0, 60)}"`);
    });

  test(`${provider}: a rejected read stays unread and is shown as refused`, { skip: !enabled, timeout: 300_000 }, async () => {
    const secret = join(root, `secret-${provider}.txt`);
    await writeFile(secret, 'THE-SECRET-IS-ZEBRA-9921');
    const reader = await agent(provider, `Reader ${provider}`, 'You read files with the miniclaw tool. Be brief.', {}, true);
    const thread = await chat(provider, reader.id);
    const done = await converse(thread.id, provider,
      `Use your miniclaw tool to read_file ${secret} and tell me what it says. If you cannot, say CANNOT.`, { approve: 'reject' });
    assert.equal(JSON.stringify(done).includes('ZEBRA-9921'), false, 'the content never left the file');
    const refused = done.activities.find(entry => entry.kind === 'read');
    assert.equal(refused?.status, 'refused');
    note(`${provider} refused read`, `asked ${done.seen.approvals.length} time(s), refused, content absent; said "${done.answer.slice(0, 60)}"`);
  });
}

for (const provider of ['codex', 'claude']) {
  test(`${provider}: in a coding session a real edit waits for review, a rejection with a note is obeyed, and Apply writes it`,
    { skip: !enabled, timeout: 500_000 }, async () => {
      const file = join(work, `session-${provider}.txt`);
      await writeFile(file, 'colour = red\n');
      const coder = await agent(provider, `Coder ${provider}`, 'You edit files with the miniclaw tool. Be brief.', {}, true);
      const thread = await chat(provider, coder.id);
      await json(await post(`/api/permissions/conversation/${thread.id}`,
        { target: { target_type: 'feature', name: 'coding_session' }, effect: 'allow' }));
      const reviews = [];
      await json(await post(`/api/threads/${thread.id}/messages`, { clientMessageId: crypto.randomUUID(),
        model: PROVIDERS[provider].model, effort: PROVIDERS[provider].effort ?? undefined,
        text: `Use your miniclaw tool to change "red" to "blue" in ${file}. If a change is rejected, follow the user's note and try again.` }), 202);
      const done = await eventually(async () => {
        const value = await state(thread.id);
        if (value.review && !reviews.some(seen => seen.id === value.review.id)) {
          reviews.push({ ...value.review, onDisk: await readFile(file, 'utf8') });
          // The first proposal is rejected with a note; the second is applied.
          await post(`/api/threads/${thread.id}/reviews/${value.review.id}`, reviews.length === 1
            ? { action: 'reject', note: 'Use green instead of blue.' } : { action: 'apply' });
        }
        return value;
      }, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant' && reviews.length >= 1,
      'the coding session turn', 400_000);
      assert.ok(reviews.length >= 2, `the agent tried again after the rejection: ${reviews.length} review(s); said ${done.messages.at(-1).text}`);
      assert.ok(reviews.every(review => review.onDisk === 'colour = red\n'), 'nothing was written while a review was open');
      assert.match(reviews[0].diff, /^\+colour = blue$/m);
      assert.match(reviews[1].diff, /^\+colour = green$/m, 'the note reached the agent');
      assert.equal(await readFile(file, 'utf8'), 'colour = green\n');
      note(`${provider} coding session`, `${reviews.length} reviews; rejected "blue" with a note, agent proposed "green", applied`);
    });
}

for (const provider of ['codex', 'claude']) {
  test(`${provider}: an agent creates a new file, reviewed before it exists, and does not claim the folder is read-only`,
    { skip: !enabled, timeout: 400_000 }, async () => {
      const file = join(work, `created-${provider}.txt`);
      const maker = await agent(provider, `Maker ${provider}`, 'You create and edit files with the miniclaw tool. Be brief.', {}, true);
      const thread = await chat(provider, maker.id);
      await json(await post(`/api/permissions/conversation/${thread.id}`,
        { target: { target_type: 'feature', name: 'coding_session' }, effect: 'allow' }));
      const reviews = [];
      await json(await post(`/api/threads/${thread.id}/messages`, { clientMessageId: crypto.randomUUID(),
        model: PROVIDERS[provider].model, effort: PROVIDERS[provider].effort ?? undefined,
        text: `Create the file ${file} containing exactly the line: hello from ${provider}` }), 202);
      const done = await eventually(async () => {
        const value = await state(thread.id);
        if (value.review && !reviews.some(seen => seen.id === value.review.id)) {
          reviews.push({ ...value.review, existed: await readFile(file, 'utf8').then(() => true, () => false) });
          await post(`/api/threads/${thread.id}/reviews/${value.review.id}`, { action: 'apply' });
        }
        if (value.scopedApproval) await post(`/api/threads/${thread.id}/scoped-approvals/${value.scopedApproval.id}`, { action: 'reject' });
        return value;
      }, value => !value.activeTurn && !value.pending && value.messages.at(-1)?.role === 'assistant', 'the turn', 300_000);
      assert.equal(reviews.length, 1, `one review: ${done.messages.at(-1).text}`);
      assert.deepEqual([reviews[0].kind, reviews[0].existed], ['create', false]);
      assert.match(await readFile(file, 'utf8'), new RegExp(`hello from ${provider}`));
      assert.doesNotMatch(done.messages.at(-1).text, /read-only|read only/i);
      note(`${provider} creates a file`, `reviewed as "create" before it existed; said "${done.messages.at(-1).text.slice(0, 60)}"`);
    });
}

// Why 60 minutes: since 2026-10-11 a delegated run's limit counts in MiniClaw minutes, which `before` shortens to
// 5 s (MINICLAW_TEST_MINUTE_MS) so an approval wait can be seen to end. The default 10 minutes would then stop a real
// review after 50 s. 60 of them are 300 s, five real minutes: room for a real provider's review, still inside these
// tests' 300 to 500 s timeouts. 60 is also the most a waited run is waited for, so no run here is handed to the
// background at the hour (only a limit above 60 is), and the waited tests still get the answer in the tool call.
const LONG_RUN = { maxPerTurn: 3, minutes: 60 };
const reviewer = provider => agent(provider, `Reviewer ${provider} ${crypto.randomUUID().slice(0, 4)}`,
  'You review short texts. Reply in one sentence and always include the word PINEAPPLE.');

for (const [from, to] of [['codex', 'codex'], ['codex', 'claude'], ['claude', 'codex']]) {
  test(`delegation, wait: a ${from} agent delegates to a ${to} agent in the middle of a conversation`,
    { skip: !enabled, timeout: 500_000 }, async () => {
      const recipient = await reviewer(to);
      const lead = await agent(from, `Lead ${from}-${to}`, `You lead. When asked to get a review, call your delegation tool with the agent "${recipient.name}".`,
        { delegatesTo: [recipient.id], delegation: LONG_RUN });
      const thread = await chat(from, lead.id);
      await converse(thread.id, from, 'Remember this sentence: "Teh cat sat on the mat." Reply only with OK.');
      const done = await converse(thread.id, from, `Ask ${recipient.name} to review that sentence for typos, then tell me exactly what it said.`);
      assert.equal(done.seen.delegations.length, 1, 'the user was asked once');
      const [run] = done.delegations;
      assert.deepEqual([run.agent, run.mode, run.status], [recipient.name, 'wait', 'done']);
      assert.match(run.result, /PINEAPPLE/i, 'the recipient ran under its own instructions');
      assert.match(done.answer, /PINEAPPLE/i, 'its answer reached the lead agent');
      const { threads } = await json(await app.request('/api/threads'));
      assert.equal(threads.some(task => task.id === run.thread), false, 'the run is not a conversation of its own');
      note(`delegation ${from} -> ${to} (wait)`, `${Math.round((run.ended - run.started) / 1000)} s; recipient said "${run.result.slice(0, 70)}"`);
    });
}

test('delegation, background: a codex lead carries on, and learns the result with the next message',
  { skip: !enabled, timeout: 500_000 }, async () => {
    const recipient = await reviewer('codex');
    const lead = await agent('codex', 'Lead background', `You lead. When asked to get a review, call your delegation tool with the agent "${recipient.name}".`,
      { delegatesTo: [recipient.id], delegation: LONG_RUN });
    const thread = await chat('codex', lead.id);
    const first = await converse(thread.id, 'codex', `Ask ${recipient.name} to review this sentence for typos: "Teh dog." Then reply to me.`, { delegate: 'background' });
    assert.doesNotMatch(first.answer, /PINEAPPLE/i, 'the lead did not wait');
    await eventually(() => state(thread.id), value => value.delegations[0]?.status === 'done', 'the background run to finish', 240_000);
    // The idle lead carries on with the result by itself (delegation::carry_on); let that turn end.
    await eventually(() => state(thread.id), value => !value.activeTurn && !value.pending, 'the carry-on turn to end', 240_000);
    const later = await converse(thread.id, 'codex', `What did ${recipient.name} say? Quote it.`);
    assert.match(later.answer, /PINEAPPLE/i);
    note('delegation codex background', `lead first said "${first.answer.slice(0, 50)}", later quoted the result`);
  });

test("a delegated agent's approval appears in the parent, names it, and uses its own permissions",
  { skip: !enabled, timeout: 500_000 }, async () => {
    const file = join(work, 'for-the-reader.txt');
    await writeFile(file, 'The launch code word is MANGO.');
    // The lead may use the work folder; the recipient has no grant at all.
    const recipient = await agent('codex', 'Ungranted reader', 'You read files with the miniclaw tool and report what they say, briefly.');
    const lead = await agent('codex', 'Lead with grant', `You lead. To read files, delegate to the agent "${recipient.name}".`,
      { delegatesTo: [recipient.id] }, true);
    const thread = await chat('codex', lead.id);
    const done = await converse(thread.id, 'codex', `Ask ${recipient.name} to read ${file} and report its content. Then tell me what it said.`,
      { approve: 'approve_once' });
    const asked = done.seen.approvals.find(approval => approval.agent === recipient.name);
    assert.ok(asked, `the recipient had to ask although the lead holds a grant: ${JSON.stringify(done.seen.approvals)}`);
    assert.ok(asked.thread && asked.thread !== thread.id, 'the answer goes to the run, not the parent');
    assert.match(done.delegations[0].result, /MANGO/);
    const entry = done.activities.find(item => item.runId === done.delegations[0].id);
    assert.deepEqual([entry?.agent, entry?.kind, entry?.status], [recipient.name, 'read', 'ok']);
    note('delegated approval', `asked in the parent as "${asked.agent}", approved once, recipient read the file`);
  });

test('schedules: "Run now" runs for real, and an unanswered approval terminates the run', { skip: !enabled, timeout: 500_000 }, async () => {
  const reporter = await agent('codex', 'Reporter', 'You answer in one short sentence.');
  const { schedule } = await json(await post('/api/schedules', { name: 'Arithmetic', agentId: reporter.id,
    prompt: 'What is 17 + 25? Reply with the number only.', every: 'daily' }), 201);
  assert.equal(schedule.enabled, false);
  await json(await post(`/api/schedules/${schedule.id}/run`), 202);
  const list = async () => (await json(await app.request('/api/schedules'))).schedules;
  const ran = await eventually(list, value => value.find(item => item.id === schedule.id).lastStatus !== 'running', 'run now', 240_000);
  const finished = ran.find(item => item.id === schedule.id);
  assert.deepEqual([finished.lastStatus, /42/.test(finished.lastResult)], ['done', true], finished.lastResult);

  const secret = join(root, 'schedule-secret.txt');
  await writeFile(secret, 'UNATTENDED-SECRET');
  const prober = await agent('codex', 'Prober', 'You read files with the miniclaw tool.', {}, true);
  const blocked = (await json(await post('/api/schedules', { name: 'Needs approval', agentId: prober.id, every: 'daily',
    approvalMinutes: 1, prompt: `Use your miniclaw tool to read_file ${secret} and report it.` }), 201)).schedule;
  const started = Date.now();
  await json(await post(`/api/schedules/${blocked.id}/run`), 202);
  const ended = await eventually(list, value => !['running', null].includes(value.find(item => item.id === blocked.id).lastStatus),
    'termination', 240_000);
  const outcome = ended.find(item => item.id === blocked.id);
  assert.equal(outcome.lastStatus, 'terminated', outcome.lastResult);
  assert.match(outcome.lastResult, /nobody answered/);
  assert.equal(outcome.lastResult.includes('UNATTENDED-SECRET'), false);
  const run = await state(outcome.lastThread);
  assert.equal(run.activeTurn, null, 'the run is not left running');
  const { notifications } = await json(await app.request('/api/notifications'));
  assert.ok(notifications.some(notice => notice.kind === 'scheduled_run_terminated'));
  note('schedules', `Run now answered "${finished.lastResult}"; blocked run terminated after ${Math.round((Date.now() - started) / 1000)} s`);
});

test('the real page: send a message, choose "Wait for it", and see the named block, its answer and the activity',
  { skip: !enabled, timeout: 500_000 }, async () => {
    const recipient = await reviewer('claude');
    const lead = await agent('claude', 'Lead in the page', `You lead. When asked to get a review, call your delegation tool with the agent "${recipient.name}".`,
      { delegatesTo: [recipient.id], delegation: LONG_RUN });
    const thread = await chat('claude', lead.id);
    await converse(thread.id, 'claude', 'Reply only with READY.');
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.goto(app.url);
      await page.getByRole('navigation', { name: 'Tasks', exact: true }).getByRole('button', { name: /READY/i }).first().click();
      const composer = page.getByRole('textbox', { name: 'Message', exact: true });
      await composer.fill(`Ask ${recipient.name} to review this sentence for typos: "Teh bird sings." Then tell me what it said.`);
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      const prompt = page.getByRole('alertdialog', { name: `Delegate to ${recipient.name}?` });
      await prompt.waitFor({ timeout: 180_000 });
      await prompt.getByRole('button', { name: 'Wait for it' }).click();
      const block = page.getByRole('group', { name: `Delegated to ${recipient.name}` });
      await block.getByText('Finished').waitFor({ timeout: 240_000 });
      const answer = await block.getByRole('article', { name: `Answer from ${recipient.name}` }).innerText();
      assert.match(answer, /PINEAPPLE/i);
      // The parent's own reply comes after, under its own label.
      await page.getByRole('article', { name: 'Assistant message', exact: true }).last().getByText(/PINEAPPLE/i).waitFor({ timeout: 180_000 });
      await block.getByRole('button', { name: 'Show the full run' }).click();
      await block.getByText(/Teh bird sings/).first().waitFor({ timeout: 30_000 });
      await page.screenshot({ path: join(process.env.MINICLAW_E2E_SHOTS ?? root, 'real-delegation.png'), fullPage: true });
      assert.deepEqual(errors, [], 'the page threw no error');
      note('real page', `prompt answered by click; block showed "${answer.slice(0, 60)}"`);
    } finally { await browser.close(); }
  });

for (const provider of ['codex', 'claude']) {
  test(`delegation progress: ${provider} reads the saved run status`, { skip: !enabled, timeout: 300_000 }, async () => {
    const recipient = await agent(provider, `Progress reviewer ${provider}`, 'Reply with exactly PROGRESS-REVIEW-OK.');
    const lead = await agent(provider, `Progress lead ${provider}`, 'Use the delegation tool to delegate tasks and to check their status.',
      { delegatesTo: [recipient.id], delegation: LONG_RUN });
    const task = await chat(provider, lead.id);
    const first = await converse(task.id, provider,
      `Delegate to ${recipient.name} the task: reply with PROGRESS-REVIEW-OK. Use the delegation tool.`, { delegate: 'wait' });
    assert.equal(first.delegations.length, 1);
    assert.equal(first.delegations[0].status, 'done');
    const followup = await converse(task.id, provider,
      'What is the progress on that delegated task? Call miniclaw_delegate with action status. Report the exact run id and status from its result. Do not delegate again.');
    assert.match(followup.answer, new RegExp(first.delegations[0].id));
    assert.match(followup.answer, /done|finished|completed/i);
    assert.equal(followup.delegations.length, 1);
    note(`${provider} delegation progress`, 'follow-up returned the real run id and finished status without delegating again');
  });
}
