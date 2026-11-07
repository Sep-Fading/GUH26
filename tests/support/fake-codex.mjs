#!/usr/bin/env node
// Scripted upstream peer. It has no MiniClaw HTTP endpoints or UI logic.
import { readFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { createInterface } from 'node:readline';

if (process.argv.includes('--version')) {
  process.stdout.write('codex-cli 0.154.0-alpha.6.1\n');
  process.exit(0);
}

const scenario = JSON.parse(readFileSync(process.env.MC_SCENARIO, 'utf8'));
const handlers = new Map(Object.entries(scenario.handlers ?? {}));
const requests = [];
const responses = [];
const held = new Map();
let initialized = false;
let acknowledged = false;
const send = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);

function receive(message) {
  if (!message.method) {
    responses.push(message);
    return;
  }
  requests.push(message);
  if (message.method === 'initialize') {
    if (initialized) return send({ id: message.id, error: { code: -32600, message: 'Already initialized' } });
    initialized = true;
    send({ id: message.id, result: {
      userAgent: 'miniclaw-test-peer', platformFamily: 'unix', platformOs: 'macos',
      codexHome: process.env.CODEX_HOME ?? '/test/codex',
    } });
    return;
  }
  if (message.method === 'initialized') {
    acknowledged = initialized;
    return;
  }
  if (!acknowledged) return send({ id: message.id, error: { code: -32600, message: 'Not initialized' } });
  const configured = handlers.get(message.method);
  const handler = configured?.cases?.find(entry => Object.entries(entry.params)
    .every(([key, value]) => JSON.stringify(message.params?.[key]) === JSON.stringify(value)))
    ?.handler ?? configured;
  if (!handler) return send({ id: message.id, error: { code: -32601, message: `Unscripted method: ${message.method}` } });
  for (const frame of handler.before ?? []) send(frame);
  if (handler.hold) held.set(String(message.id), message);
  else if (handler.error) send({ id: message.id, error: handler.error });
  else send({ id: message.id, result: handler.result ?? {} });
  for (const frame of handler.after ?? []) send(frame);
  if (handler.exit) process.exit(0);
}

const input = createInterface({ input: process.stdin });
input.on('line', (line) => {
  try { receive(JSON.parse(line)); }
  catch { send({ id: null, error: { code: -32700, message: 'Invalid JSON' } }); }
});
input.on('close', () => process.exit(0));

const control = createServer((socket) => {
  const lines = createInterface({ input: socket });
  lines.on('line', (line) => {
    try {
      const command = JSON.parse(line);
      let result = {};
      switch (command.op) {
        case 'shutdown': socket.end(`${JSON.stringify({ result: {} })}\n`, () => process.exit(0)); return;
        case 'inspect': result = { requests, responses, held: [...held.values()], initialized, acknowledged, codexHome: process.env.CODEX_HOME }; break;
        case 'set': handlers.set(command.method, command.handler); break;
        case 'emit': for (const frame of command.frames) send(frame); break;
        case 'release': {
          const key = String(command.id);
          if (!held.has(key)) throw new Error('Request is not held');
          send({ id: held.get(key).id, ...command.reply });
          held.delete(key);
          break;
        }
        case 'raw': process.stdout.write(command.text); break;
        default: throw new Error(`Unknown test control: ${command.op}`);
      }
      socket.end(`${JSON.stringify({ result })}\n`);
    } catch (error) { socket.end(`${JSON.stringify({ error: error.message })}\n`); }
  });
});
control.listen(process.env.MC_CONTROL);
