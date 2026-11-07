// Checks generated launch settings and restart behavior.
import assert from 'node:assert/strict';
// Runs isolated worker service regression cases.
import { test } from 'node:test';
// Executes native property-list tools without shells.
import { execFileSync } from 'node:child_process';
// Creates disposable agent fixtures and private logs.
import { copyFile, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
// Resolves paths inside each isolated fixture.
import { join } from 'node:path';
// Locates temporary storage outside real installations.
import { tmpdir } from 'node:os';
// Bounds observation of launchd process restarts.
import { eventually } from './support/harness.mjs';

// Executes the actual plist builder without running the guest installer entry point.
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'mc-worker-agent-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const path = join(root, 'agent.plist');
  const source = await readFile('scripts/vm/install-master.command', 'utf8');
  assert.ok(source.endsWith('\ninstall_codex\n'));
  const definitions = source.slice(0, -'install_codex\n'.length);
  execFileSync('/bin/bash', ['-c', definitions + '\nwrite_agent "$@"',
    'fixture', path, join(root, 'worker with spaces'), join(root, 'worker.json'), root]);
  const plist = JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', path],
    { encoding: 'utf8' }));
  return { root, path, plist };
}

test('worker job restarts failures but leaves normal exits stopped', {
  skip: process.platform === 'win32' && 'macOS VM guest only: a launchd property list written by the bash guest installer and read with plutil',
}, async t => {
  const { root, plist } = await fixture(t);
  assert.equal(plist.Label, 'dev.miniclaw.worker');
  assert.deepEqual(plist.ProgramArguments, [join(root, 'worker with spaces'), join(root, 'worker.json')]);
  assert.deepEqual(plist.KeepAlive, { SuccessfulExit: false });
  assert.equal(plist.ThrottleInterval, 10);
  assert.equal(plist.RunAtLoad, true);
  assert.equal(plist.UserName, undefined);
  assert.equal(plist.EnvironmentVariables.HOME, process.env.HOME);
});

if (process.env.MINICLAW_TEST_LAUNCHD === '1') {
  test('launchd replaces a failed worker without a second bootstrap', { timeout: 40_000 }, async t => {
    const { root, path } = await fixture(t);
    const label = `dev.miniclaw.test.${root.split('/').at(-1)}`;
    const domain = `gui/${process.getuid()}`;
    const marker = join(root, 'runs');
    const program = join(root, 'peer.mjs');
    await writeFile(program, `import { appendFileSync } from 'node:fs';\n`
      + `appendFileSync(${JSON.stringify(marker)}, 'started\\n');\nprocess.exit(1);\n`);
    execFileSync('/usr/bin/plutil', ['-replace', 'Label', '-string', label, path]);
    execFileSync('/usr/bin/plutil', ['-replace', 'ProgramArguments', '-json',
      JSON.stringify([process.execPath, program]), path]);
    execFileSync('/bin/launchctl', ['bootstrap', domain, path]);
    try {
      await eventually(async () => (await readFile(marker, 'utf8').catch(() => '')).trim().split('\n'),
        lines => lines.filter(line => line === 'started').length >= 2,
        'launchd restart after failure', 30_000);
    } finally {
      execFileSync('/bin/launchctl', ['bootout', `${domain}/${label}`]);
    }
  });
}
