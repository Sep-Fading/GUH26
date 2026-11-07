// Creates distinct names for disposable staged receipts.
import { randomUUID } from 'node:crypto';
// Publishes private fixture files without replacing receipts.
import { closeSync, constants, fstatSync, linkSync, lstatSync, openSync,
  unlinkSync, writeSync } from 'node:fs';
// Resolves bounded filenames inside fixture directories.
import { join } from 'node:path';

// Classifies injected proof only; no result authorizes production resource release.
export function classifyRecovery(evidence) {
  const answer = classification => ({ classification, productionRelease: false });
  const bounded = value => typeof value === 'string' && value.length > 0 && value.length <= 256;
  if (!evidence || !bounded(evidence.lease) || !bounded(evidence.attempt)
    || !['master', 'controller'].includes(evidence.role)
    || evidence.fenceHeld !== true || evidence.guestLockHeld !== true
    || typeof evidence.identity !== 'string' || !evidence.identity
    || evidence.identity !== evidence.observedIdentity) return answer('unknown');
  if (evidence.phase === 'prepared') return answer('never_started');
  const receipt = evidence.receipt;
  if (evidence.phase !== 'spawn_intent' || evidence.publicationConfirmed !== true
    || receipt?.version !== 1 || receipt.lease !== evidence.lease
    || receipt.attempt !== evidence.attempt || receipt.role !== evidence.role
    || receipt.identity !== evidence.identity) return answer('unknown');
  if (receipt.proof === 'guest_did_stop') return answer('confirmed_stop');
  if (receipt.proof === 'force_stop_success') return answer('forced_stop');
  return answer('unknown');
}

// Publishes one bounded fixture after file flush; errors never confirm publication.
export function publishReceipt(directory, name, value, { syncFile, syncDirectory }) {
  if (!/^[a-z][a-z0-9-]{0,63}\.json$/.test(name)
    || typeof syncFile !== 'function' || typeof syncDirectory !== 'function') {
    throw new Error('Invalid fixture publication');
  }
  const encoded = JSON.stringify(value);
  if (!encoded || Buffer.byteLength(encoded) > 4096) throw new Error('Invalid receipt size');
  const dir = openSync(directory, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  const staged = join(directory, `.pending-${randomUUID()}`), final = join(directory, name);
  let file;
  try {
    const owner = fstatSync(dir);
    if (!owner.isDirectory() || owner.uid !== process.getuid() || (owner.mode & 0o077)) {
      throw new Error('Fixture directory must be private');
    }
    file = openSync(staged, constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW
      | constants.O_RDWR, 0o600);
    const bytes = Buffer.from(encoded);
    for (let offset = 0; offset < bytes.length;) {
      const count = writeSync(file, bytes, offset, bytes.length - offset);
      if (!count) throw new Error('Incomplete receipt write');
      offset += count;
    }
    syncFile(file);
    const current = lstatSync(directory);
    if (current.dev !== owner.dev || current.ino !== owner.ino || !current.isDirectory()) {
      throw new Error('Fixture directory changed');
    }
    linkSync(staged, final);
    unlinkSync(staged);
    syncDirectory(dir);
    return final;
  } finally {
    if (file !== undefined) closeSync(file);
    closeSync(dir);
    try { unlinkSync(staged); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
}
