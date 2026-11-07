// Verifies real startup and persisted browser conversations.
import assert from 'node:assert/strict';
// Runs explicitly selected paired guest acceptance checks.
import { test } from 'node:test';
// Drives the packaged interface without API mocks.
import { chromium, expect } from '@playwright/test';
// Runs installed commands and inspects owned listeners.
import { spawn, execFileSync } from 'node:child_process';
// Reads receipts and retains private test reports.
import { lstat, mkdir, readFile, writeFile } from 'node:fs/promises';
// Resolves explicit installation and evidence locations.
import { isAbsolute, join } from 'node:path';
// Bounds readiness polling without delaying other tasks.
import { setTimeout as delay } from 'node:timers/promises';
// Rejects occupied test ports before starting guests.
import { checkPort } from '../scripts/setup/system.mjs';
// Records forced shutdown separately from clean exits.
import { shutdownResult } from './support/vm-shutdown.mjs';

// Returns the selected process and its descendants, not unrelated host services.
function processTree(pid) {
  const rows = execFileSync('/bin/ps', ['-axo', 'pid=,ppid='], { encoding: 'utf8' })
    .trim().split('\n').map(row => row.trim().split(/\s+/).map(Number));
  const ids = new Set([pid]);
  let size;
  do {
    size = ids.size;
    for (const [child, parent] of rows) if (ids.has(parent)) ids.add(child);
  } while (ids.size !== size);
  return [...ids];
}

test('installed VM pair serves, streams and restores without silent errors', async t => {
  const root = process.env.MINICLAW_E2E_INSTALL;
  assert.ok(root && isAbsolute(root), 'Set MINICLAW_E2E_INSTALL to an explicit test installation');
  const info = await lstat(join(root, 'settings.json'));
  assert.ok(info.isFile() && info.uid === process.getuid() && !(info.mode & 0o077));
  const settings = JSON.parse(await readFile(join(root, 'settings.json'), 'utf8'));
  assert.equal(settings.schemaVersion, 1);
  assert.equal(settings.backend, 'linux');
  assert.equal(settings.vmBackendReady, true, 'Complete guest onboarding in miniclaw setup first');
  assert.equal(settings.pair?.root, root);
  assert.equal(settings.pair.port, settings.port);
  assert.ok(Number.isInteger(settings.port) && settings.port >= 1024 && settings.port <= 65535);
  assert.notEqual(settings.port, 8787, 'Use a separate test installation, not the current app');
  const base = `http://127.0.0.1:${settings.port}`;
  const attached = process.env.MINICLAW_E2E_RUNNING === '1';
  const verifyStartup = process.env.MINICLAW_E2E_STARTUP === '1';
  const evidence = join(root, `e2e-${Date.now()}`);
  await mkdir(evidence, { mode: 0o700 });
  const report = { mode: attached ? 'already-running' : 'installed-two-boots',
    passed: false, stages: [], shutdowns: [], failures: [] };
  let browser, context, threadId;
  // Records timings and failure stage without private message or credential content.
  async function stage(name, work) {
    const began = Date.now();
    try { await work(); report.stages.push({ name, passed: true, ms: Date.now() - began }); }
    catch (error) {
      report.stages.push({ name, passed: false, ms: Date.now() - began });
      throw error;
    }
  }
  try {
    browser = await chromium.launch();
    context = await browser.newContext();
    // Observes real SSE event names without changing requests or response data.
    await context.addInitScript(() => {
      window.miniclawTestEvents = [];
      const Source = window.EventSource;
      window.EventSource = class extends Source {
        // Records delivery and error events without inspecting their payloads.
        constructor(...args) {
          super(...args);
          for (const name of ['message.delta', 'turn.completed', 'turn.error', 'error']) {
            this.addEventListener(name, () => window.miniclawTestEvents.push(name));
          }
        }
      };
    });
    for (let boot = 0; boot < (attached ? 1 : 2); boot++) {
      let child, exited, output = '', page, bootError;
      try {
        if (!attached) {
          await checkPort(settings.port);
          child = spawn(process.execPath, ['scripts/cli.mjs', 'start', '--installation', root],
            { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, MINICLAW_VM_DIAGNOSTICS: '0' } });
          exited = new Promise(resolve => {
            child.once('error', () => resolve(1));
            child.once('close', code => resolve(code ?? 1));
          });
          for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => {
            output = (output + chunk).slice(-8192);
          });
          t.diagnostic(`Boot ${boot + 1}: unlock the Master VM when its login screen appears.`);
        }
        await stage(`boot-${boot + 1}-readiness`, async () => {
          const began = Date.now();
          let sawWaiting = false;
          while (Date.now() - began < 126_000) {
            if (child) assert.ok(child.exitCode === null && child.signalCode === null, output);
            const reading = await (async () => {
              const response = await fetch(`${base}/api/health`, { redirect: 'error',
                signal: AbortSignal.timeout(2000) });
              const health = await response.json();
              return { response, health };
            })().catch(() => null);
            if (reading) {
              const { response, health } = reading;
              if (verifyStartup && boot === 0 && !sawWaiting
                && health.startup === 'waiting_for_worker') {
                const waiting = await context.newPage();
                try {
                  await waiting.goto(base, { waitUntil: 'domcontentloaded' });
                  await expect(waiting.getByRole('status')).toHaveText('Waiting for Master VM');
                  await expect(waiting.getByRole('alert')).toContainText('Unlock the Master VM');
                  await expect(waiting.getByRole('textbox', { name: 'Message', exact: true })).toBeDisabled();
                  await waiting.screenshot({ path: join(evidence, 'waiting.png') });
                  sawWaiting = true;
                  report.stages.push({ name: 'waiting-interface', passed: true, ms: Date.now() - began });
                  console.error('Waiting interface verified. Please unlock the Master VM now.');
                } finally { await waiting.close(); }
              }
              if (response.ok && health.backend === 'ok' && health.codex === 'connected'
                && (!child || output.includes(`MiniClaw is ready at ${base}`))) {
                assert.ok(!verifyStartup || boot !== 0 || sawWaiting,
                  'Master connected before the waiting interface was verified');
                return;
              }
            }
            await delay(200);
          }
          assert.fail('VM readiness exceeded 126 seconds');
        });
        await stage(`boot-${boot + 1}-private-interface`, async () => {
          if (child) {
            const sockets = execFileSync('/usr/sbin/lsof', ['-nP', '-a', '-p',
              processTree(child.pid).join(','), '-iTCP', '-sTCP:LISTEN', '-Fn'], { encoding: 'utf8' });
            assert.deepEqual(sockets.split('\n').filter(line => line.startsWith('n')), [`n127.0.0.1:${settings.port}`]);
          }
          const blocked = await fetch(`${base}/api/health`, { headers: { Origin: 'https://invalid.example' },
            redirect: 'error', signal: AbortSignal.timeout(5000) });
          assert.equal(blocked.status, 403);
        });
        page = await context.newPage();
        page.setDefaultTimeout(15_000);
        const faults = [];
        page.on('pageerror', () => faults.push('Uncaught browser exception'));
        page.on('console', message => { if (message.type() === 'error') faults.push('Browser console error'); });
        page.on('response', response => {
          if (response.url().startsWith(base + '/') && response.status() >= 400) {
            faults.push(`HTTP ${response.status()} ${new URL(response.url()).pathname}`);
          }
        });
        page.on('requestfailed', request => {
          if (request.url().startsWith(base + '/')) faults.push(`Request failed: ${new URL(request.url()).pathname}`);
        });
        await stage(`boot-${boot + 1}-browser`, async () => {
          await page.goto(base, { waitUntil: 'domcontentloaded' });
          await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
          if (boot === 0) {
            await page.getByRole('button', { name: 'New chat', exact: true }).first().click();
            const model = page.getByRole('combobox', { name: 'Model', exact: true });
            await expect(model).toBeEnabled();
            await model.click();
            await page.getByRole('option', { name: 'GPT-6-Astra', exact: true }).click();
            await page.getByRole('combobox', { name: 'Reasoning effort', exact: true }).click();
            await page.getByRole('option', { name: 'low', exact: true }).click();
            await page.getByRole('textbox', { name: 'Message', exact: true })
              .fill('Reply exactly VM_E2E_OK. Do not use tools or access files.');
            const sent = page.waitForResponse(response => /\/api\/threads\/[^/]+\/messages$/.test(response.url())
              && response.request().method() === 'POST');
            await page.getByRole('button', { name: 'Send', exact: true }).click();
            const response = await sent;
            assert.equal(response.status(), 202);
            threadId = new URL(response.url()).pathname.split('/')[3];
          }
          await expect(page.getByRole('article', { name: 'Assistant message' }).last())
            .toHaveText('VM_E2E_OK', { timeout: 90_000 });
          await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
          const snapshot = await page.request.get(`${base}/api/threads/${threadId}`);
          assert.equal(snapshot.status(), 200);
          const saved = await snapshot.json();
          assert.equal(saved.activeTurn, null);
          assert.equal(saved.messages.at(-1).text.trim(), 'VM_E2E_OK');
          const events = await page.evaluate(() => window.miniclawTestEvents);
          if (boot === 0) {
            assert.ok(events.includes('message.delta'), 'No streamed assistant delta reached the browser');
            assert.ok(events.includes('turn.completed'), 'No completed turn reached the browser');
          }
          assert.ok(!events.includes('turn.error') && !events.includes('error'), 'SSE reported an error');
          await expect(page.getByRole('alert')).toHaveCount(0);
          assert.deepEqual(faults, []);
          await page.screenshot({ path: join(evidence, `boot-${boot + 1}.png`) });
        });
      } catch (error) {
        bootError = error;
      } finally {
        await page?.close();
        if (child) {
          const stopAt = Date.now();
          child.kill('SIGTERM');
          const reminder = setInterval(() => console.error(
            'Waiting for native exit; each guest is forced off after 30 seconds if needed.'), 30_000);
          try {
            await stage(`boot-${boot + 1}-shutdown`, async () => {
              const result = shutdownResult(await exited, output, Date.now() - stopAt);
              report.shutdowns.push({ boot: boot + 1, ...result });
              await checkPort(settings.port);
              await assert.rejects(lstat(join(root, 'ActiveSession')), { code: 'ENOENT' });
              t.diagnostic(`Boot ${boot + 1}: ${result.forced ? 'forced' : 'clean'} shutdown in ${result.ms} ms`);
            });
          } catch (error) {
            if (bootError) throw new AggregateError([bootError, error], 'Boot and shutdown both failed');
            throw error;
          } finally { clearInterval(reminder); }
        }
      }
      if (bootError) throw bootError;
    }
    report.passed = true;
  } catch (error) {
    report.failures.push({ name: error.name,
      stage: report.stages.filter(stage => !stage.passed).at(-1)?.name ?? 'lifecycle' });
    throw error;
  } finally {
    await browser?.close();
    await writeFile(join(evidence, 'report.json'), JSON.stringify(report, null, 2), { mode: 0o600 });
    t.diagnostic(`Private evidence: ${evidence}`);
  }
});
