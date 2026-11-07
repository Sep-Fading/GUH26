// Measures when Chromium draws a frame of a multipart JPEG stream.
import assert from 'node:assert/strict';
// Runs the browser measurement as one case.
import { test } from 'node:test';
// Serves the stream variants to the browser.
import { createServer } from 'node:http';
// Drives the same Chromium the browser tests use.
import { chromium } from '@playwright/test';

const open = Buffer.from('--frame\r\nContent-Type: image/jpeg\r\n\r\n');

// The desktop stream sends a frame and may then send nothing for a long time, when the VM
// screen is still. This records which stream shape makes the browser show that last frame.
test('Chromium draws a stream frame only once the next part has been opened', { timeout: 60_000 }, async t => {
  const browser = await chromium.launch();
  t.after(() => browser.close());
  const page = await browser.newPage({ viewport: { width: 300, height: 120 } });
  // Two plain JPEGs made in the browser: red is the older frame, blue the newest.
  const [red, blue] = (await page.evaluate(() => ['#d00', '#00d'].map(colour => {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 64;
    const context = canvas.getContext('2d');
    context.fillStyle = colour;
    context.fillRect(0, 0, 64, 64);
    return canvas.toDataURL('image/jpeg').split(',')[1];
  }))).map(data => Buffer.from(data, 'base64'));
  const server = createServer((request, response) => {
    if (request.url === '/') {
      response.setHeader('Content-Type', 'text/html');
      return response.end('<body style="margin:0;display:flex">'
        + ['bare', 'opened', 'blue'].map(name => `<img id="${name}" width="64" height="64" src="/${name}">`).join(''));
    }
    if (request.url === '/blue') { response.setHeader('Content-Type', 'image/jpeg'); return response.end(blue); }
    response.writeHead(200, { 'Content-Type': 'multipart/x-mixed-replace; boundary=frame' });
    const opened = request.url === '/opened';
    // "bare" ends a frame with a line break only; "opened" also opens the next part at once.
    const send = frame => response.write(opened ? Buffer.concat([frame, Buffer.from('\r\n'), open])
      : Buffer.concat([open, frame, Buffer.from('\r\n')]));
    if (opened) response.write(open);
    send(red);
    setTimeout(() => send(blue), 300);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => server.close());
  await page.goto(`http://127.0.0.1:${server.address().port}/`, { waitUntil: 'commit' });
  await page.waitForTimeout(2500);
  const shot = async id => (await page.locator(`#${id}`).screenshot()).toString('base64');
  const [bare, withOpening, reference] = [await shot('bare'), await shot('opened'), await shot('blue')];
  assert.notEqual(bare, reference, 'a frame that only ends with a line break stays undrawn');
  assert.equal(withOpening, reference, 'opening the next part makes the newest frame visible');
});
