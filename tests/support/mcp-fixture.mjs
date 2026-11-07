// Places the stdio MCP test server in a folder and says how to start it.
import { access, copyFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { command } from './harness.mjs';

// The Mac suites run `fake-mcp.mjs` with Node. Node cannot start inside the Windows sandbox yet
// (it initialises Winsock at startup, which an offline sandboxed run is refused), so Windows runs
// the same server built as a native program: `cargo build --example mcp_fixture`.
// MINICLAW_TEST_NATIVE_MCP=1 selects the native one elsewhere, to compare the two.
export const nativeMcp = process.platform === 'win32' || process.env.MINICLAW_TEST_NATIVE_MCP === '1';

/** Copies the server into `folder`; `args(mode)` are the arguments that start it in a mode. */
export async function placeMcpServer(folder) {
  if (!nativeMcp) {
    await copyFile(new URL('./fake-mcp.mjs', import.meta.url), join(folder, 'server.mjs'));
    return { command: process.execPath, file: 'server.mjs', args: (...mode) => [join(folder, 'server.mjs'), ...mode] };
  }
  const suffix = process.platform === 'win32' ? '.exe' : '';
  const built = join(dirname(command[0]), 'examples', `mcp_fixture${suffix}`);
  try { await access(built); }
  catch { throw new Error(`The native MCP test server is not built: ${built}. Run cargo build --example mcp_fixture.`); }
  const file = `mcp-server-fixture${suffix}`;
  await copyFile(built, join(folder, file));
  return { command: join(folder, file), file, args: (...mode) => mode };
}
