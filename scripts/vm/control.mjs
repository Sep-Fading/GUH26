// Verifies role-bound requests without exposing the key.
import { createHmac, timingSafeEqual } from 'node:crypto';
// Accepts one private request per Unix connection.
import { createServer } from 'node:net';
// Resolves the owned control socket path.
import { join } from 'node:path';
// Reuses strict private socket and directory checks.
import { listen, privateDirectory } from './broker.mjs';

const publicReasons = new Set([
  'VM resource limit exceeded.',
  'Master VM network disconnected.',
  'Master VM stopped before it was ready.',
  'Master VM resource ownership is unresolved.',
  'Master VM setup is incomplete.',
  'Master VM could not start. Check VM diagnostics.',
  'Assigned VM does not exist.',
  'Project VM setup was cancelled. Its files were kept for retry.',
]);
const uuid = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;

// Requests about the shared VM budget rather than one VM.
const ledgerMethods = ['ledger', 'limits', 'reconcile', 'forget'];

// Parses "cpu,memory,disk" as whole percentages from 1 to 100, or returns null.
function percentages(text) {
  if (typeof text !== 'string' || !/^\d{1,3},\d{1,3},\d{1,3}$/.test(text)) return null;
  const [cpuPercent, memoryPercent, diskPercent] = text.split(',').map(Number);
  if ([cpuPercent, memoryPercent, diskPercent].some(value => value < 1 || value > 100)) return null;
  return { cpuPercent, memoryPercent, diskPercent };
}

// Serves authenticated Master status and start requests from the Mac worker.
export async function startControl(dir, key, actions) {
  await privateDirectory(dir);
  if (!Buffer.isBuffer(key) || key.length !== 32
    || typeof actions?.status !== 'function' || typeof actions?.start !== 'function') {
    throw new Error('Invalid VM control configuration');
  }
  const used = new Set();
  const sockets = new Set();
  const starting = new Map();
  const server = createServer(socket => {
    sockets.add(socket);
    socket.setTimeout(5000, () => socket.destroy());
    socket.on('error', () => {});
    socket.on('close', () => sockets.delete(socket));
    let line = '', answered = false;
    socket.on('data', chunk => {
      if (answered) { socket.destroy(); return; }
      line += chunk.toString();
      if (line.length > 512) { socket.destroy(); return; }
      const end = line.indexOf('\n');
      if (end < 0) return;
      answered = true;
      if (end !== line.length - 1) { socket.destroy(); return; }
      void (async () => {
        let reply = { ok: false, error: { code: 'denied', message: 'VM control request denied.' } };
        let vmId = 'master';
        let method;
        try {
          const value = JSON.parse(line.slice(0, end));
          const nonce = value?.nonce;
          method = value?.method;
          vmId = value?.vmId ?? 'master';
          const requestId = value?.requestId;
          if (!/^[a-f0-9]{64}$/.test(nonce)
            || !['status', 'start', 'stop', 'show', 'list', 'provision', 'retry', 'cancel',
              'delete', ...ledgerMethods].includes(method)
            // The budget requests name no VM; only `limits` carries percentages, and they are signed.
            || (ledgerMethods.includes(method) && (value.vmId !== undefined
              || (method === 'limits') !== (value.limits !== undefined)))
            || (value.limits !== undefined && !percentages(value.limits))
            || !/^[a-f0-9]{64}$/.test(value?.mac)
            || typeof vmId !== 'string'
            || (['list', 'provision'].includes(method) && value.vmId !== undefined)
            || (['retry', 'cancel', 'delete'].includes(method) && !uuid.test(value.vmId))
            || (method === 'provision' ? !uuid.test(requestId) : requestId !== undefined)
            || (vmId !== 'master' && !uuid.test(vmId))
            || Object.keys(value).some(field =>
              !['nonce', 'method', 'mac', 'vmId', 'requestId', 'limits'].includes(field))) {
            throw new Error('Denied');
          }
          const expected = createHmac('sha256', key)
            .update(`${nonce}:${method}${value.vmId ? `:${vmId}` : ''}`
              + `${requestId ? `:${requestId}` : ''}${value.limits ? `:${value.limits}` : ''}`).digest();
          if (!timingSafeEqual(expected, Buffer.from(value.mac, 'hex')) || used.has(nonce)) {
            throw new Error('Denied');
          }
          used.add(nonce);
          if (used.size > 1024) used.delete(used.values().next().value);
          if (ledgerMethods.includes(method)) {
            if (typeof actions[method] !== 'function') throw new Error('Unavailable');
            reply = { ok: true, ledger: await actions[method](percentages(value.limits) ?? undefined) };
          }
          else if (method === 'list') reply = { ok: true, machines: actions.list?.() ?? [] };
          else if (method === 'status') reply = { ok: true, status: actions.status(vmId) };
          else if (method === 'stop' || method === 'show') {
            if (actions.status(vmId)?.state !== 'running'
              || typeof actions[method] !== 'function') throw new Error('Unavailable');
            reply = { ok: true, status: await actions[method](vmId) };
          }
          else if (method === 'delete') {
            // Only a VM that is neither running nor starting may lose its disk.
            if (['running', 'starting', 'stopping'].includes(actions.status(vmId)?.state)
              || starting.has(vmId) || typeof actions.delete !== 'function') throw new Error('Unavailable');
            reply = { ok: true, status: await actions.delete(vmId) };
          }
          else if (method === 'provision') {
            if (typeof actions.provision !== 'function') throw new Error('Unavailable');
            reply = { ok: true, machine: await actions.provision(requestId) };
          } else if (method === 'retry' || method === 'cancel') {
            if (typeof actions[method] !== 'function') throw new Error('Unavailable');
            reply = { ok: true, status: await actions[method](vmId) };
          }
          else {
            if (!starting.has(vmId)) {
              const request = Promise.resolve().then(() => actions.start(vmId))
                .finally(() => starting.delete(vmId));
              starting.set(vmId, request);
            }
            reply = { ok: true, status: await starting.get(vmId) };
          }
        } catch (error) {
          if (error?.message !== 'Denied') {
            const reason = actions.status(vmId)?.reason;
            reply = { ok: false, error: { code: method === 'start' ? 'start_failed' : 'setup_failed',
              message: publicReasons.has(reason) ? reason
                : method === 'provision' && error?.message === 'Project VM limit reached'
                  ? 'Project VM limit reached.'
                : method === 'start'
                  ? vmId === 'master' ? 'Master VM could not start. Check VM diagnostics.'
                    : 'Project VM could not start. Check VM diagnostics.'
                  : 'Project VM setup could not proceed. Check its status and host capacity.' } };
          }
        }
        socket.end(`${JSON.stringify(reply)}\n`);
      })();
    });
  });
  try { await listen(server, join(dir, 'control.sock')); }
  catch (error) { server.close(); throw error; }
  return {
    // Stops admission and completes existing control calls before cleanup.
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise(resolve => server.close(resolve));
    },
  };
}
