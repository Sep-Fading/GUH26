// Reads the installation's network client configuration and the service's status file.
import { readFile } from 'node:fs/promises';
// Builds Windows paths independently of the host running the tests.
import { win32 } from 'node:path';
// Restarts the installed service from the reviewed backend with one administrator approval.
import { updateWindowsNetwork } from './windows-network.mjs';

// Says what the service's status file reports, in one sentence for the terminal.
export function describeAccess(status) {
  if (status?.removed === true) return { ok: false, text: 'The network service removed its Windows access entries.' };
  if (status?.ok === true) {
    return { ok: true, text: Array.isArray(status.repaired) && status.repaired.length
      ? `Network access for sandboxed tools is set up again (${status.repaired.length} Windows settings restored).`
      : 'Network access for sandboxed tools is set up.' };
  }
  const reason = typeof status?.error === 'string' && status.error ? status.error.replace(/[\x00-\x1f]/g, ' ').slice(0, 300) : 'no status was published';
  return { ok: false, text: `The network service could not set up network access for sandboxed tools: ${reason}` };
}

// Sets up the network service's Windows access again: restarts the installed service from this
// installation's reviewed backend (one administrator approval), which checks and repairs its
// entries before it reports running, then reads what it published.
export async function repairWindows(destination, { update = updateWindowsNetwork, read = readFile,
  programData = process.env.ProgramData || 'C:\\ProgramData', onOutput = text => process.stdout.write(text), platform = process.platform } = {}) {
  if (platform !== 'win32') throw new Error('miniclaw repair sets up the Windows network service again; this computer does not use it.');
  const runtime = win32.join(destination, 'runtime'), data = win32.join(destination, 'data');
  // Said first: without a path the remembered installation is the one repaired.
  onOutput(`Repairing the MiniClaw installation at ${destination}.\n`);
  const config = JSON.parse(await read(win32.join(data, 'windows-network.json'), 'utf8').catch(error => {
    if (error.code !== 'ENOENT') throw error;
    throw new Error('The MiniClaw network service is not installed. Run `miniclaw resume` to finish setup, or `miniclaw setup` to install again.');
  }));
  if (!/^MiniClawNetwork-[a-f0-9]{32}$/.test(config.service_name ?? '')) throw new Error('Invalid installed network service identity');
  await update({ runtime, data, onOutput, config });
  const status = JSON.parse(await read(win32.join(programData, config.service_name, 'winsock-acl-status.json'), 'utf8'));
  const result = describeAccess(status);
  onOutput(`${result.text}\n`);
  if (!result.ok) throw new Error(result.text);
  return result;
}
