// A release carries compiled files, so a Mac without Rust, Swift or Go can install MiniClaw.
// A checkout carries none and builds them. Every prebuilt file is checked against the
// checksum its release recorded before it is used.
// Hashes a file before it is trusted.
import { createHash } from 'node:crypto';
// Reads the manifest and the files.
import { readFile } from 'node:fs/promises';
// Resolves names inside the prebuilt folder.
import { join } from 'node:path';
// Locates the folder beside the scripts.
import { fileURLToPath } from 'node:url';

// Opens the prebuilt folder of one MiniClaw root: a release has it, a checkout does not.
export function prebuiltIn(root) {
  const folder = join(root, 'prebuilt');
  let manifest;
  // Reads the manifest once; a missing one means "build from source".
  async function load() {
    if (manifest !== undefined) return manifest;
    let candidate;
    try { candidate = JSON.parse(await readFile(join(folder, 'manifest.json'), 'utf8')); }
    catch (error) { if (error.code !== 'ENOENT') throw error; candidate = null; }
    if (candidate && (candidate.platform ?? 'darwin') !== process.platform) {
      throw new Error(`This release platform is ${candidate.platform ?? 'darwin'}, not ${process.platform}.`);
    }
    if (candidate && candidate.arch !== process.arch) {
      throw new Error(`This release was built for ${candidate.arch}, and this ${process.platform === 'win32' ? 'computer' : 'Mac'} is ${process.arch}.`);
    }
    manifest = candidate;
    return manifest;
  }
  return {
    // Reports whether this root is a release.
    async available() { return (await load()) !== null; },
    // Returns a verified prebuilt file, or null in a checkout. In a release a missing or
    // changed file is an error: it never falls back to compiling without saying so.
    async file(name) {
      const listed = await load();
      if (!listed) return null;
      const expected = listed.files?.[name];
      if (typeof expected !== 'string') throw new Error(`${name} is not part of this release.`);
      const path = join(folder, name);
      const actual = createHash('sha256').update(await readFile(path)).digest('hex');
      if (actual !== expected) throw new Error(`${name} does not match the checksum recorded in this release.`);
      return path;
    },
    folder,
  };
}

// The prebuilt folder of the MiniClaw these scripts belong to. In a checkout or a release the
// scripts sit two levels below the root (`scripts/setup/`); in an installation one level
// (`setup/`). The first root that has a manifest is used.
const roots = ['../../', '../'].map(step => prebuiltIn(fileURLToPath(new URL(step, import.meta.url))));
async function chosen() {
  for (const root of roots) if (await root.available()) return root;
  return roots[0];
}
export const prebuilt = {
  available: async () => (await chosen()).available(),
  file: async name => (await chosen()).file(name),
  get folder() { return roots[0].folder; },
};
