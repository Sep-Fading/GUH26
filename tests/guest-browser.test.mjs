// Checks the offline browser fixture independently.
import assert from 'node:assert/strict';
// Runs the fixture before native experiments.
import { test } from 'node:test';
// Reads the exact guest test document.
import { readFile } from 'node:fs/promises';
// Exercises controls without a listening server.
import { chromium } from '@playwright/test';

// Verifies visible input, canvas hit testing and no network dependencies.
test('guest browser fixture requires input and a target hit', async () => {
  const html = await readFile(new URL('./fixtures/guest-browser.html', import.meta.url), 'utf8');
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    const requests = [];
    page.on('request', request => requests.push(request.url()));
    await page.setContent(html);
    assert.equal(await page.locator('output').innerText(), 'Waiting');
    await page.getByLabel('Test value', { exact: true }).fill('73');
    await page.getByRole('button', { name: 'Apply', exact: true }).click();
    assert.equal(await page.locator('output').innerText(), 'Select the blue square');
    const canvas = page.getByLabel('Visual target', { exact: true });
    await canvas.click({ position: { x: 30, y: 30 } });
    await page.getByRole('button', { name: 'Apply', exact: true }).click();
    assert.equal(await page.locator('output').innerText(), 'Select the blue square');
    await canvas.click({ position: { x: 180, y: 70 } });
    await page.getByRole('button', { name: 'Apply', exact: true }).click();
    assert.equal(await page.locator('output').innerText(), 'Passed: 73');
    assert.deepEqual(requests, []);
  } finally { await browser.close(); }
});
