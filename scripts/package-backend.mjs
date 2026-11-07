// Runs native builds without invoking shells.
import { execFile } from 'node:child_process';
// Creates temporary builds, checks destinations and reads the built seed library.
import { lstat, mkdtemp, readFile, rm } from 'node:fs/promises';
// Hashes the seed library the backend is built to accept.
import { createHash } from 'node:crypto';
// Locates temporary storage for isolated builds.
import { tmpdir } from 'node:os';
// Resolves build roots and output directories.
import { join, resolve } from 'node:path';
// Converts script URLs into filesystem paths.
import { fileURLToPath } from 'node:url';
// Awaits build commands and their output.
import { promisify } from 'node:util';
// Compiles frontend assets without starting listeners.
import { build } from 'vite';
// Copies declared runtime files into packages.
import { assemble } from './backend-package.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const exec = promisify(execFile);

// Runs Cargo offline and returns its stdout, preserving build diagnostics on failure.
async function cargo(args) {
  try {
    const result = await exec('cargo', [...args, '--locked', '--offline'], {
      cwd: root, maxBuffer: 32 * 1024 * 1024,
    });
    if (result.stderr) process.stderr.write(result.stderr);
    return result.stdout;
  } catch (error) {
    if (error.stderr) process.stderr.write(error.stderr);
    throw error;
  }
}

// Builds a native host bundle without changing live assets or installing services.
async function main() {
  const args = process.argv.slice(2);
  // `--host` is accepted for older callers; native host packaging is the only kind.
  if (args[0] === '--host') args.shift();
  if (args.length !== 1 || args[0].startsWith('-')) {
    throw new Error('Usage: npm run package:backend -- NEW_OUTPUT_DIRECTORY');
  }
  const output = resolve(args[0]);
  const exists = await lstat(output).then(() => true, error => {
    if (error.code === 'ENOENT') return false;
    throw error;
  });
  if (exists) throw new Error('Output already exists; choose a new directory');
  const temp = await mkdtemp(join(tmpdir(), 'mc-build-'));
  try {
    const metadata = JSON.parse(await cargo(['metadata', '--no-deps', '--format-version=1']));
    const pkg = metadata.packages.find(value => value.name === 'miniclaw-server');
    if (!pkg) throw new Error('Cargo did not report the backend package');
    // On Windows the OpenSSL seed library of the private Node is built first; the backend is
    // built with its hash, so it accepts exactly this library beside its private Node.
    let seedLibrary;
    if (process.platform === 'win32') {
      console.log('Building the OpenSSL seed library...');
      const seedMessages = await cargo(['build', '--release', '--manifest-path', join(root, 'openssl-seed', 'Cargo.toml'),
        '--target-dir', join(temp, 'openssl-seed'), '--message-format=json-render-diagnostics']);
      seedLibrary = seedMessages.trim().split('\n').map(line => JSON.parse(line))
        .find(value => value.reason === 'compiler-artifact' && value.target?.name === 'miniclaw_openssl_seed')
        ?.filenames?.find(name => name.toLowerCase().endsWith('.dll'));
      if (!seedLibrary) throw new Error('Cargo did not produce the OpenSSL seed library');
      process.env.MINICLAW_OPENSSL_SEED_SHA256 = createHash('sha256').update(await readFile(seedLibrary)).digest('hex');
    }
    console.log('Building the backend release...');
    const messages = await cargo(['build', '--release', '--package', 'miniclaw-server',
      '--message-format=json-render-diagnostics']);
    const artifact = messages.trim().split('\n').map(line => JSON.parse(line))
      .find(value => value.reason === 'compiler-artifact'
        && value.target?.name === 'miniclaw-server' && value.executable);
    if (!artifact) throw new Error('Cargo did not produce the backend executable');
    const assets = join(temp, 'web');
    await build({ root: join(root, 'web'), build: { outDir: assets, emptyOutDir: true } });
    const manifest = await assemble({ binary: artifact.executable, assets, output,
      version: pkg.version, seedLibrary });
    console.log(`Packaged ${manifest.version} (${manifest.platform}/${manifest.arch}): ${output}`);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

try {
  await main();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
