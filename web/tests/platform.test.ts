// Checks that wording follows the machine MiniClaw runs on, and that macOS keeps its own.
import { afterEach, expect, test, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { host, setHost, words } from '../src/platform.svelte';
import { fileName, lastParts } from '../src/paths';
import { languageOf } from '../src/diff';
import AppearanceSettings from '../src/AppearanceSettings.svelte';
import App from '../src/App.svelte';
import PermissionSettings from '../src/PermissionSettings.svelte';
import type { Chat } from '../src/chat.svelte';

afterEach(() => { setHost('macos'); vi.unstubAllGlobals(); });

test('macOS keeps the wording it had before the Windows port', () => {
  setHost('macos');
  expect(words.machine).toBe('this Mac');
  expect(words.matchSystem).toBe('Match macOS');
  expect(words.secretStore).toBe('the macOS Keychain');
  expect(words.secretStoreShort).toBe('the Keychain');
  expect(words.account).toBe('your macOS user');
  expect(words.recovery).toBe('the Trash on this Mac, or Transfers/discarded in a VM');
  expect(words.commandPlaceholder).toBe('/opt/homebrew/bin/npx');
  expect(words.appleScript).toBe(true);
});

test('Windows gets neutral wording and no Mac-only choices', () => {
  setHost('windows');
  expect(words.machine).toBe('this computer');
  expect(words.matchSystem).toBe('Match system');
  expect(words.secretStore).toBe('Windows Credential Manager');
  expect(words.account).toBe('your user account');
  expect(words.recovery).toMatch(/Recovered files/);
  expect(words.recovery).not.toMatch(/Trash|Mac/);
  expect(words.commandPlaceholder).not.toMatch(/homebrew/);
  expect(words.appleScript).toBe(false);
});

test('Linux gets neutral wording, its own paths and no Mac-only choices', () => {
  setHost('linux');
  expect(host.os).toBe('linux');
  expect(words.machine).toBe('this computer');
  expect(words.matchSystem).toBe('Match system');
  expect(words.secretStore).toBe("MiniClaw's private data folder");
  expect(words.secretStoreShort).toBe("MiniClaw's private data folder");
  expect(words.account).toBe('your user account');
  expect(words.recovery).toBe("MiniClaw's Recovered files folder");
  expect(words.folderTrash).toBe("MiniClaw's Recovered files folder");
  expect(words.commandPlaceholder).toBe('/usr/bin/npx');
  expect(words.folderPlaceholder).toBe('/home/you/mcp/notes');
  expect(words.appleScript).toBe(false);
  // Linux paths split on slashes only, as on the Mac.
  expect(fileName('/home/me/site/odd\\name.txt')).toBe('odd\\name.txt');
});

test('only a platform the backend names is accepted', () => {
  setHost('windows');
  setHost('freebsd');
  setHost(undefined);
  expect(host.os).toBe('windows');
});

test('the appearance radio follows the platform and keeps its macOS name', async () => {
  setHost('macos');
  const first = render(AppearanceSettings);
  expect(screen.getByRole('radio', { name: 'Match macOS' })).toBeInTheDocument();
  expect(screen.getByText(/Saved on this Mac,/)).toBeInTheDocument();
  first.unmount();
  setHost('windows');
  render(AppearanceSettings);
  expect(screen.getByRole('radio', { name: 'Match system' })).toBeInTheDocument();
  expect(screen.queryByRole('radio', { name: 'Match macOS' })).toBeNull();
  expect(screen.getByText(/Saved on this computer,/)).toBeInTheDocument();
});

test('Windows paths split on either separator; a macOS backslash stays part of the name', () => {
  expect(fileName('C:\\Users\\Owner Ω\\site\\src\\app.rs')).toBe('app.rs');
  expect(lastParts('C:\\Users\\Owner\\site\\src\\app.rs', 2)).toBe('src\\app.rs');
  expect(fileName('C:/Users/Owner/site/app.rs')).toBe('app.rs');
  expect(fileName('\\\\server\\share\\notes.md')).toBe('notes.md');
  expect(languageOf('C:\\work\\src\\main.rs')).toBe('rust');
  setHost('macos');
  expect(fileName('/Users/me/site/odd\\name.txt')).toBe('odd\\name.txt');
  expect(lastParts('/Users/me/site/src/app.js', 2)).toBe('src/app.js');
  expect(fileName('src/app.js')).toBe('app.js');
  // A relative Windows path has no drive to recognise, so the backend's platform decides.
  setHost('windows');
  expect(fileName('src\\app.js')).toBe('app.js');
  expect(lastParts('site\\src\\app.js', 2)).toBe('src\\app.js');
  expect(languageOf('src\\main.rs')).toBe('rust');
});

test('the health reply tells the page which platform the backend runs on', async () => {
  setHost('macos');
  vi.stubGlobal('fetch', (url: string) => url === '/api/health' ? Promise.resolve(Response.json({ backend: 'ok', platform: 'windows' }))
    : url === '/api/account' ? Promise.resolve(Response.json({ status: 'unsupported_auth' }))
      : Promise.resolve(new Response('', { status: 503 })));
  render(App);
  await vi.waitFor(() => expect(host.os).toBe('windows'));
});

// Windows has no VMs, and the backend answers their status with 503: the page must not ask.
test('the page asks for VM status on the Mac only', async () => {
  for (const os of ['windows', 'macos'] as const) {
    setHost(os);
    const urls: string[] = [];
    vi.stubGlobal('fetch', (url: string) => {
      urls.push(String(url));
      return url === '/api/health' ? Promise.resolve(Response.json({ backend: 'ok', platform: os }))
        : url === '/api/account' ? Promise.resolve(Response.json({ status: 'unsupported_auth' }))
          : Promise.resolve(new Response('', { status: 503 }));
    });
    const view = render(App);
    await vi.waitFor(() => expect(urls).toContain('/api/health'));
    await new Promise(resolve => setTimeout(resolve, 50));
    const asked = urls.filter(url => /^\/api\/vms\/[^/]+\/status/.test(url));
    if (os === 'windows') expect(asked).toEqual([]);
    else expect(asked).toContain('/api/vms/master/status');
    view.unmount();
    vi.unstubAllGlobals();
  }
});

// The permission text names the sandbox of the machine MiniClaw runs on; the Mac keeps its words.
test('permission settings name the Windows sandbox on Windows and Seatbelt on the Mac', async () => {
  vi.stubGlobal('fetch', () => Promise.resolve(Response.json({ available: true, access: { state: 'healthy' } })));
  const chat = { permissionSettings: () => new Promise(() => {}), mcp: () => Promise.resolve({ servers: [] }) } as unknown as Chat;
  setHost('macos');
  const mac = render(PermissionSettings, { chat, kind: 'agent', id: 'a1' });
  expect(mac.container.textContent).toContain('Shell commands run in a Seatbelt sandbox limited to the allowed files and folders.');
  mac.unmount();
  setHost('windows');
  const windows = render(PermissionSettings, { chat, kind: 'agent', id: 'a1' });
  const text = windows.container.textContent ?? '';
  expect(text).not.toMatch(/Seatbelt|sandbox-exec|this Mac/);
  expect(text).toContain("Shell commands run in MiniClaw's Windows sandbox, limited to the allowed files and folders.");
  windows.unmount();
});
