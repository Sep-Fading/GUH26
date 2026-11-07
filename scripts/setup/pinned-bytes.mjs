// Checks downloaded and offline pinned dependency bytes.
import { createHash } from 'node:crypto';
// Reads bounded input through one retained handle.
import { open } from 'node:fs/promises';

// Bounds package downloads before accepting their pinned checksum.
async function download(pin) {
  const response = await fetch(pin.url, { signal: AbortSignal.timeout(120000) });
  if (!response.ok || !response.body) throw new Error(`${pin.name} download failed: HTTP ${response.status}`);
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > pin.bytes) throw new Error(`${pin.name} exceeds pinned size`);
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

// Rejects oversized offline input before allocating its contents, retaining one file handle.
async function offline(path, pin) {
  const file = await open(path, 'r');
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size !== pin.bytes) throw new Error(`${pin.name} checksum mismatch`);
    const bytes = Buffer.alloc(pin.bytes + 1);
    let offset = 0;
    while (offset < bytes.length) {
      const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset);
      if (!bytesRead) break;
      offset += bytesRead;
    }
    return bytes.subarray(0, offset);
  } finally { await file.close(); }
}

// Loads bounded bytes and verifies integrity before any decoding or packaging.
export async function readPinned(pin, { archive, bytes } = {}) {
  const content = bytes ?? (archive ? await offline(archive, pin) : await download(pin));
  if (content.length !== pin.bytes || createHash('sha256').update(content).digest('hex') !== pin.digest) {
    throw new Error(`${pin.name} checksum mismatch`);
  }
  return content;
}
