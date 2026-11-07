// Checks verified downloads and partial-file recovery.
import assert from 'node:assert/strict';
// Runs isolated download lifecycle acceptance cases.
import { test } from 'node:test';
// Computes expected hashes independently from downloader.
import { createHash } from 'node:crypto';
// Creates disposable caches and inspects results.
import { mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
// Locates temporary storage for download fixtures.
import { tmpdir } from 'node:os';
// Resolves files beneath each isolated cache.
import { join } from 'node:path';
// Exercises download verification without internet access.
import { cachedItems, downloadImage } from '../scripts/setup/download.mjs';

const data = Buffer.from('a small verified image');
const image = { id: 'linux', file: 'test.img', url: 'https://cloud.debian.org/test.img',
  bytes: data.length, algorithm: 'sha256', digest: createHash('sha256').update(data).digest('hex') };

// Allocates a private cache removed after its test.
async function cache(t) {
  const directory = await mkdtemp(join(tmpdir(), 'mc-download-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

test('verified downloads are reused without another request', async t => {
  const dir = await cache(t);
  let calls = 0;
  const progress = [];
  const fetcher = async () => { calls++; return new Response(data); };
  const first = await downloadImage(image, dir, { fetcher, onProgress: value => progress.push(value) });
  assert.deepEqual(await readFile(first.path), data);
  assert.equal(first.cached, false);
  const second = await downloadImage(image, dir, { fetcher });
  assert.equal(second.cached, true);
  assert.equal(calls, 1);
  assert.equal(progress.at(-1).received, data.length);
  assert.equal(progress.at(-1).verified, true);
});

test('official Codex packages use the same checksum gate', async t => {
  const dir = await cache(t);
  const packageImage = { ...image,
    url: 'https://releases.openai.com/codex/releases/0.154.0/package.tar.gz' };
  const result = await downloadImage(packageImage, dir, {
    fetcher: async () => new Response(data),
  });
  assert.deepEqual(await readFile(result.path), data);
});

test('checksum failure never promotes a partial file', async t => {
  const dir = await cache(t);
  await assert.rejects(downloadImage(image, dir, {
    fetcher: async () => new Response(Buffer.alloc(data.length)),
  }), /checksum/i);
  assert.deepEqual(await readdir(dir), []);
});

test('interrupted downloads resume only at the declared offset', async t => {
  const dir = await cache(t);
  await writeFile(join(dir, `${image.file}.part`), data.subarray(0, 5));
  const result = await downloadImage(image, dir, { fetcher: async (_url, options) => {
    assert.equal(options.headers.Range, 'bytes=5-');
    return new Response(data.subarray(5), { status: 206,
      headers: { 'Content-Range': `bytes 5-${data.length - 1}/${data.length}` } });
  } });
  assert.deepEqual(await readFile(result.path), data);
});

test('a server ignoring Range restarts without duplicate bytes', async t => {
  const dir = await cache(t);
  await writeFile(join(dir, `${image.file}.part`), data.subarray(0, 5));
  const result = await downloadImage(image, dir, { fetcher: async () => new Response(data) });
  assert.deepEqual(await readFile(result.path), data);
});

test('wrong ranges and oversized bodies fail before promotion', async t => {
  const dir = await cache(t);
  await writeFile(join(dir, `${image.file}.part`), data.subarray(0, 5));
  await assert.rejects(downloadImage(image, dir, { fetcher: async () => new Response(data, {
    status: 206, headers: { 'Content-Range': `bytes 0-${data.length - 1}/${data.length}` },
  }) }), /range/i);
  await rm(join(dir, `${image.file}.part`));
  await assert.rejects(downloadImage(image, dir, {
    fetcher: async () => new Response(Buffer.alloc(data.length + 1)),
  }), /size/i);
  assert.ok(!(await readdir(dir)).includes(image.file));
});

test('cancellation retains partial bytes and releases ownership', async t => {
  const dir = await cache(t);
  const controller = new AbortController();
  await assert.rejects(downloadImage(image, dir, { signal: controller.signal,
    onProgress: ({ received }) => { if (received) controller.abort(); },
    fetcher: async () => new Response(new ReadableStream({ start(stream) {
      stream.enqueue(data.subarray(0, 5));
      stream.enqueue(data.subarray(5));
      stream.close();
    } })),
  }), /abort/i);
  assert.deepEqual(await readFile(join(dir, `${image.file}.part`)), data.subarray(0, 5));
  assert.ok(!(await readdir(dir)).some(file => file.endsWith('.lock')));
});

test('linked files and existing download owners are refused', async t => {
  const dir = await cache(t);
  const outside = join(dir, 'keep');
  await writeFile(outside, 'keep');
  await symlink(outside, join(dir, `${image.file}.part`));
  await assert.rejects(downloadImage(image, dir), /regular/i);
  assert.equal(await readFile(outside, 'utf8'), 'keep');
  await rm(join(dir, `${image.file}.part`));
  await writeFile(join(dir, `${image.file}.lock`), 'other owner');
  await assert.rejects(downloadImage(image, dir), /EEXIST|in progress/i);
  assert.equal(await readFile(join(dir, `${image.file}.lock`), 'utf8'), 'other owner');
});

test('redirects cannot target local or insecure endpoints', async t => {
  const dir = await cache(t);
  for (const location of ['http://cloud.debian.org/image', 'https://127.0.0.1/image']) {
    await assert.rejects(downloadImage(image, dir, { fetcher: async () =>
      new Response(null, { status: 302, headers: { Location: location } }),
    }), /download host/i);
  }
});

// The pinned Go archive may redirect only to its official download host.
test('network toolchain follows the official Go redirect', async t => {
  const dir = await cache(t);
  const calls = [];
  await downloadImage({ ...image, url: 'https://go.dev/dl/go.tar.gz' }, dir, {
    fetcher: async url => {
      calls.push(url.hostname);
      return calls.length === 1
        ? new Response(null, { status: 302, headers: { Location: 'https://dl.google.com/go/go.tar.gz' } })
        : new Response(data);
    },
  });
  assert.deepEqual(calls, ['go.dev', 'dl.google.com']);
});

test('cached items are recognised by exact name and size, never by a link', async t => {
  const cache = await mkdtemp(join(tmpdir(), 'mc-cached-'));
  t.after(() => rm(cache, { recursive: true, force: true }));
  const items = [{ id: 'macos', file: 'macos.ipsw', bytes: 5 }, { id: 'codex', file: 'codex.tgz', bytes: 5 },
    { id: 'go', file: 'go.tgz', bytes: 5 }, { id: 'linked', file: 'linked.bin', bytes: 5 }];
  await writeFile(join(cache, 'macos.ipsw'), '12345');
  await writeFile(join(cache, 'codex.tgz'), '1234');
  await symlink(join(cache, 'macos.ipsw'), join(cache, 'linked.bin'));
  assert.deepEqual([...await cachedItems(items, cache)], ['macos']);
  assert.deepEqual([...await cachedItems(items, join(cache, 'missing'))], []);
});

test('only the official Node download folder is accepted from nodejs.org', async t => {
  const { downloadImage } = await import('../scripts/setup/download.mjs');
  const { mkdtemp, rm, chmod } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const cache = await mkdtemp(join(tmpdir(), 'mc-node-host-'));
  t.after(() => rm(cache, { recursive: true, force: true }));
  await chmod(cache, 0o700);
  const item = url => ({ file: 'node.tar.gz', url, bytes: 10, algorithm: 'sha256', digest: 'a'.repeat(64) });
  const never = async () => { throw new Error('reached the network'); };
  await assert.rejects(downloadImage(item('https://nodejs.org/evil/node.tar.gz'), cache, { fetcher: never }), /Unapproved download host/);
  await assert.rejects(downloadImage(item('https://nodejs.org.evil.example/dist/v24/node.tar.gz'), cache, { fetcher: never }), /Unapproved download host/);
  await assert.rejects(downloadImage(item('https://nodejs.org/dist/v24.21.0/node.tar.gz'), cache, { fetcher: never }), /reached the network/);
});

// Windows directory privacy is an ACL decision, never synthesized Unix mode bits.
test('Windows download cache requires native ACL verification', async t => {
  const { validateCacheDirectory } = await import('../scripts/setup/download.mjs');
  const dir = await cache(t);
  let checked = false;
  await validateCacheDirectory(dir, { platform: 'win32', verifyWindows: async path => {
    assert.equal(path, dir); checked = true;
  } });
  assert.ok(checked);
  await assert.rejects(validateCacheDirectory(dir, { platform: 'win32', verifyWindows: async () => {
    throw new Error('Cache grants another account access');
  } }), /another account/);
});

// Exercises real Windows DACLs and a junction ancestor without changing any user cache.
test('native Windows cache accepts private ACLs and refuses broad ACLs and junctions', { skip: process.platform !== 'win32' }, async t => {
  const { mkdtemp, mkdir, rm, symlink } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join, win32 } = await import('node:path');
  const { execFile } = await import('node:child_process');
  const { promisify } = await import('node:util');
  const { validateCacheDirectory } = await import('../scripts/setup/download.mjs');
  const root = await mkdtemp(join(tmpdir(), 'mc-native-cache-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const privatePath = join(root, 'private Ω');
  const broadPath = join(root, 'broad');
  await mkdir(privatePath); await mkdir(broadPath);
  const shell = win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const literal = value => `'${value.replaceAll("'", "''")}'`;
  for (const [path, broad] of [[privatePath, false], [broadPath, true]]) {
    const script = `$ErrorActionPreference='Stop'; $user=[Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[Security.AccessControl.DirectorySecurity]::new(); $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($user); $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($user,'FullControl','ContainerInherit,ObjectInherit','None','Allow')); ${broad ? "$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new('S-1-1-0'),'ReadAndExecute','ContainerInherit,ObjectInherit','None','Allow'));" : ''} [IO.Directory]::SetAccessControl(${literal(path)},$acl)`;
    await promisify(execFile)(shell, ['-NoProfile', '-NonInteractive', '-Command', script]);
  }
  await validateCacheDirectory(privatePath);
  await assert.rejects(validateCacheDirectory(broadPath), /another account access/);
  await mkdir(join(privatePath, 'nested'));
  const junction = join(root, 'junction');
  await symlink(privatePath, junction, 'junction');
  await assert.rejects(validateCacheDirectory(join(junction, 'nested')), /reparse point/);
});

// The guard must cover fetch, hashing, promotion and every failure exit.
test('download holds cache protection until publication and releases it on error', async t => {
  const directory = await cache(t);
  let held = false, closed = 0;
  const acquireDirectory = async () => {
    held = true;
    return { close: async () => { held = false; closed++; } };
  };
  const result = await downloadImage(image, directory, { acquireDirectory, fetcher: async () => {
    assert.ok(held); return new Response(data);
  }, onProgress: progress => { if (progress.verified) assert.ok(held); } });
  assert.equal(await readFile(result.path, 'utf8'), data.toString());
  assert.equal(closed, 1);
  await assert.rejects(downloadImage({ ...image, file: 'failure.img' }, directory,
    { acquireDirectory, fetcher: async () => { throw new Error('network failed'); } }), /network failed/);
  assert.equal(closed, 2);
  assert.equal(held, false);
});

// Native handles must deny renaming both the leaf cache and its ancestor until released.
test('native Windows download cache pins block directory replacement', { skip: process.platform !== 'win32' }, async t => {
  const { mkdir, rename } = await import('node:fs/promises');
  const { acquireCacheDirectory } = await import('../scripts/setup/download.mjs');
  const root = await cache(t);
  const directory = join(root, 'cache');
  await mkdir(directory);
  const guard = await acquireCacheDirectory(directory);
  try {
    assert.equal(guard.signal.aborted, false, 'cache guard remains alive while pinned');
    await assert.rejects(rename(directory, join(root, 'moved')));
    await assert.rejects(rename(root, `${root}-moved`));
  } finally { await guard.close(); }
  await rename(directory, join(root, 'moved'));
});

// Git travels inside the release (MiniClaw's patched build); setup never downloads any Git, so the
// official MinGit release path and GitHub's release-asset host are not approved (v3 re-review R1).
test('setup never downloads Git: GitHub release paths are not approved hosts', async t => {
  const dir = await cache(t);
  let fetched = false;
  const fetcher = async () => { fetched = true; return new Response(data); };
  for (const url of ['https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.1/MinGit-2.56.0-arm64.zip',
    'https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.1/MinGit-2.56.0-64-bit.zip',
    'https://release-assets.githubusercontent.com/github-production-release-asset/MinGit-2.56.0-64-bit.zip',
    'https://github.com/untrusted/project/releases/download/latest/tool.zip',
    // MiniClaw's own release asset is fetched only through its pin (downloadReleaseAsset), never as a setup image.
    'https://github.com/Sep-Fading/miniclaw-windows/releases/download/v0.1.5/miniclaw-git-2.56.0-lpac5-arm64.zip']) {
    await assert.rejects(downloadImage({ ...image, url }, dir, { fetcher }), /Unapproved/, url);
  }
  assert.equal(fetched, false);
});

// MiniClaw's own release assets (the patched Git build and its source, for packaging only) come from that one
// repository's release paths, follow redirects only to GitHub's asset storage, and send a token only to the API.
test('release assets come only from MiniClaw\'s own GitHub release, with the pin as the gate', async () => {
  const { downloadReleaseAsset } = await import('../scripts/setup/download.mjs');
  const asset = { name: 'Git runtime', file: 'miniclaw-git-2.56.0-lpac5-arm64.zip', bytes: data.length, digest: image.digest,
    release: { repository: 'Sep-Fading/miniclaw-windows', tag: 'v0.1.5', asset: 616891706 } };
  const storage = 'https://release-assets.githubusercontent.com/github-production-release-asset/1407939257/bb05?sig=x';
  const serve = (requests, location = storage, body = data) => async (url, init = {}) => {
    requests.push({ url: String(url), auth: init.headers?.Authorization, accept: init.headers?.Accept, redirect: init.redirect });
    return requests.length === 1 ? new Response(null, { status: 302, headers: { location } }) : new Response(body);
  };
  let requests = [];
  assert.ok((await downloadReleaseAsset(asset, { fetcher: serve(requests), env: {} })).equals(data));
  assert.deepEqual(requests.map(request => request.url),
    ['https://github.com/Sep-Fading/miniclaw-windows/releases/download/v0.1.5/miniclaw-git-2.56.0-lpac5-arm64.zip', storage]);
  assert.ok(requests.every(request => request.auth === undefined && request.redirect === 'manual'));
  // A token goes to the API's asset endpoint only (a private repository serves no anonymous downloads).
  for (const env of [{ GH_TOKEN: 'test-token' }, { GITHUB_TOKEN: 'test-token' }]) {
    requests = [];
    const older = 'https://objects.githubusercontent.com/github-production-release-asset-2e65be/1/abc?X-Amz-Signature=x';
    assert.ok((await downloadReleaseAsset(asset, { fetcher: serve(requests, older), env })).equals(data));
    assert.equal(requests[0].url, 'https://api.github.com/repos/Sep-Fading/miniclaw-windows/releases/assets/616891706');
    assert.equal(requests[0].auth, 'Bearer test-token');
    assert.equal(requests[0].accept, 'application/octet-stream');
    assert.deepEqual([requests[1].url, requests[1].auth], [older, undefined]);
  }
  // Redirects anywhere else, plain HTTP, or another repository's release are refused before any request there.
  for (const location of ['https://example.com/asset.zip', 'http://release-assets.githubusercontent.com/github-production-release-asset/1/a',
    'https://release-assets.githubusercontent.com/other/1/a', 'https://github.com/other/project/releases/download/v0.1.5/miniclaw-git-2.56.0-lpac5-arm64.zip',
    'https://user:pass@release-assets.githubusercontent.com/github-production-release-asset/1/a']) {
    requests = [];
    await assert.rejects(downloadReleaseAsset(asset, { fetcher: serve(requests, location), env: {} }), /Unapproved/, location);
    assert.equal(requests.length, 1, location);
  }
  for (const release of [{ ...asset.release, repository: 'someone/else' }, { ...asset.release, tag: '../v0.1.5' }, { ...asset.release, asset: '1/../2' }]) {
    requests = [];
    await assert.rejects(downloadReleaseAsset({ ...asset, release }, { fetcher: serve(requests), env: {} }), /Unapproved|Invalid/);
    assert.equal(requests.length, 0);
  }
  // Size and SHA-256 decide: other bytes of the same length, or more bytes, are refused.
  await assert.rejects(downloadReleaseAsset(asset, { fetcher: serve([], storage, Buffer.alloc(data.length)), env: {} }), /checksum/);
  await assert.rejects(downloadReleaseAsset(asset, { fetcher: serve([], storage, Buffer.concat([data, data])), env: {} }), /exceeds|checksum/);
  // A private repository answers 404 without a token; the error says how to proceed.
  await assert.rejects(downloadReleaseAsset(asset, { fetcher: async () => new Response('', { status: 404 }), env: {} }),
    error => /HTTP 404/.test(error.message) && /GH_TOKEN/.test(error.message));
});
