// Finds provider executables without installing anything.
import { executable } from './system.mjs';
// Locates the per-user Claude installation.
import { homedir } from 'node:os';
// Builds candidate paths under the home directory.
import { join, win32 } from 'node:path';
// Asks each CLI for its own sign-in state.
import { execFile } from 'node:child_process';

// Runs one status command with a short deadline and returns its exit code and output.
function run(path, args) {
  return new Promise(resolve => {
    execFile(path, args, { timeout: 10_000, maxBuffer: 65536 }, (error, stdout) =>
      resolve({ code: error ? (error.code ?? 1) : 0, stdout: String(stdout) }));
  });
}

// Reports installed providers and whether each is signed in. Only a yes or no is
// kept; account names and credentials never enter the installer.
export async function detectProviders({ find = executable, probe = run, platform = process.platform,
  arch = process.arch, env = process.env, home = homedir() } = {}) {
  const windows = platform === 'win32';
  const pathJoin = windows ? win32.join : join;
  const triple = arch === 'arm64' ? 'aarch64-pc-windows-msvc' : 'x86_64-pc-windows-msvc';
  const prefixes = [env.APPDATA && pathJoin(env.APPDATA, 'npm'),
    ...(env.PATH ?? env.Path ?? '').split(';')].filter(Boolean);
  const npmCodex = prefixes.flatMap(prefix => [
    pathJoin(prefix, 'node_modules', '@openai', `codex-win32-${arch}`, 'vendor', triple, 'bin', 'codex.exe'),
    pathJoin(prefix, 'node_modules', '@openai', 'codex', 'node_modules', '@openai',
      `codex-win32-${arch}`, 'vendor', triple, 'bin', 'codex.exe'),
    pathJoin(prefix, 'node_modules', '@openai', 'codex', 'vendor', triple, 'codex', 'codex.exe'),
  ]);
  // npm copies the native build into a per-architecture package; its launcher stub is not used.
  const npmClaude = prefixes.flatMap(prefix => [
    pathJoin(prefix, 'node_modules', '@anthropic-ai', 'claude-code', 'node_modules', '@anthropic-ai',
      `claude-code-win32-${arch}`, 'claude.exe'),
    pathJoin(prefix, 'node_modules', '@anthropic-ai', `claude-code-win32-${arch}`, 'claude.exe'),
  ]);
  const known = [
    { id: 'codex', name: 'Codex (ChatGPT)', binary: 'codex', status: ['login', 'status'],
      candidates: windows ? [env.CODEX_BIN,
        env.LOCALAPPDATA && pathJoin(env.LOCALAPPDATA, 'Programs/OpenAI/Codex/bin/codex.exe'),
        ...npmCodex] : [env.CODEX_BIN,
        '/Applications/Codex.app/Contents/Resources/codex-cli/bin/codex',
        '/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex',
        '/Applications/Codex.app/Contents/Resources/codex',
        '/Applications/ChatGPT.app/Contents/Resources/codex'],
      signedIn: result => result.code === 0 },
    { id: 'claude', name: 'Claude Code', binary: 'claude', status: ['auth', 'status', '--json'],
      candidates: [env.CLAUDE_BIN, pathJoin(home, windows ? '.local/bin/claude.exe' : '.local/bin/claude'),
        pathJoin(home, windows ? '.claude/local/claude.exe' : '.claude/local/claude'),
        ...(windows ? [env.LOCALAPPDATA && pathJoin(env.LOCALAPPDATA, 'Microsoft/WinGet/Links/claude.exe'),
          ...npmClaude] : [])],
      signedIn: result => { try { return JSON.parse(result.stdout).loggedIn === true; }
        catch { return false; } } },
  ];
  const found = [];
  for (const provider of known) {
    const path = await find(provider.binary, provider.candidates.filter(Boolean), { platform, env });
    if (!path) continue;
    const result = await probe(path, provider.status).catch(() => ({ code: 1, stdout: '' }));
    found.push({ id: provider.id, name: provider.name, path, signedIn: provider.signedIn(result) });
  }
  return found;
}
