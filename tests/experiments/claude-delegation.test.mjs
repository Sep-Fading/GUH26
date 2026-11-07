// Runs the real Claude CLI: one Claude agent delegates, mid-conversation, to another.
import assert from 'node:assert/strict';
// Runs the check as one test case.
import { test } from 'node:test';
// Locates the signed-in CLI.
import { homedir } from 'node:os';
import { join } from 'node:path';
// Starts a disposable backend and polls its state.
import { launchApp, json, eventually } from '../support/harness.mjs';

// Real model calls on the signed-in Claude account, so it only runs when asked for.
const enabled = process.env.MINICLAW_NATIVE_MODEL_PROBE === '1';
const post = (app, path, body = {}) => app.request(path, { method: 'POST', body });
// How long the user "thinks" before answering the prompt, to show that a slow tool call survives.
const pause = Number(process.env.MINICLAW_PROBE_PAUSE_MS ?? 20_000);

for (const mode of ['wait', 'background']) {
  test(`a real Claude agent delegates to another real Claude agent (${mode})`, { skip: !enabled, timeout: 420_000 }, async t => {
    const app = await launchApp({}, { env: { CLAUDE_BIN: process.env.CLAUDE_BIN ?? join(homedir(), '.local/bin/claude') } });
    t.after(() => app.close());
    const models = (await json(await app.request('/api/models'))).models.filter(model => model.provider === 'claude');
    const model = (models.find(entry => /haiku/i.test(entry.model)) ?? models[0]).model;
    const reviewer = (await json(await post(app, '/api/agents', { name: 'Reviewer', active: true, model,
      instructions: 'You review short texts. Reply in one sentence and always include the word PINEAPPLE.' }), 201)).agent;
    const lead = (await json(await post(app, '/api/agents', { name: 'Lead', active: true, model, delegatesTo: [reviewer.id],
      instructions: 'You lead. When asked to get a review, use your delegation tool with the agent Reviewer.' }), 201)).agent;
    const { thread } = await json(await post(app, '/api/threads', { environment: { kind: 'host' }, agentId: lead.id, model }), 201);
    const state = async () => json(await app.request(`/api/threads/${thread.id}`));
    const say = text => post(app, `/api/threads/${thread.id}/messages`, { model, clientMessageId: crypto.randomUUID(), text });

    // First an ordinary exchange, so the delegation happens in the middle of a conversation.
    await json(await say('Remember this sentence: "Teh cat sat on the mat." Reply only with OK.'), 202);
    await eventually(state, value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant', 'first reply', 120_000);
    await json(await say('Ask Reviewer to review that sentence for typos, then tell me exactly what Reviewer said.'), 202);
    const asked = await eventually(state, value => value.delegationPrompt, 'the delegation prompt', 120_000);
    console.log('TASK SENT TO REVIEWER:', asked.delegationPrompt.task);
    assert.equal(asked.delegationPrompt.agent, 'Reviewer');
    await new Promise(resolve => setTimeout(resolve, pause));
    await json(await post(app, `/api/threads/${thread.id}/delegations/${asked.delegationPrompt.id}`, { mode }));
    const done = await eventually(state, value => !value.activeTurn && value.delegations[0]?.status === 'done'
      && value.messages.at(-1)?.role === 'assistant', 'both agents to finish', 300_000);
    const run = done.delegations[0];
    console.log('REVIEWER SAID:', run.result);
    console.log('LEAD SAID:', done.messages.at(-1).text);
    console.log('RUN TOOK MS:', run.ended - run.started);
    assert.match(run.result, /PINEAPPLE/i, 'the recipient ran under its own instructions');
    if (mode === 'wait') assert.match(done.messages.at(-1).text, /PINEAPPLE/i, 'the answer reached the lead agent');
    else {
      assert.doesNotMatch(done.messages.at(-1).text, /PINEAPPLE/i, 'the lead did not wait');
      await json(await say('What did Reviewer say? Quote it.'), 202);
      const later = await eventually(state, value => !value.activeTurn && value.messages.at(-1)?.role === 'assistant'
        && value.messages.length > done.messages.length, 'follow-up', 120_000);
      console.log('LEAD LATER SAID:', later.messages.at(-1).text);
      assert.match(later.messages.at(-1).text, /PINEAPPLE/i, 'the background result reached the lead with the next message');
    }
  });
}
