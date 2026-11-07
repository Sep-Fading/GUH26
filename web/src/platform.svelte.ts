// Knows which operating system MiniClaw runs on, so the page words things for that machine.
// The backend names its platform in the health reply. Until then the browser's own platform
// is the guess, which is right for a page served on loopback.

export type HostOs = 'macos' | 'windows' | 'linux';

// Reads the browser's platform; anything that is not Windows or Linux keeps the macOS wording.
function guess(): HostOs {
  const agent = typeof navigator === 'undefined' ? '' : `${navigator.platform ?? ''} ${navigator.userAgent ?? ''}`;
  if (/\bWin(dows|32|64)?\b/i.test(agent)) return 'windows';
  return /\bLinux\b/i.test(agent) && !/\bAndroid\b/i.test(agent) ? 'linux' : 'macos';
}

export const host = $state<{ os: HostOs }>({ os: guess() });

// Records the platform the backend reported; other values leave the current one unchanged.
export function setHost(os: unknown) {
  if (os === 'macos' || os === 'windows' || os === 'linux') host.os = os;
}

// Platform wording. macOS keeps the exact words it had before the Windows port.
// Linux shares Windows' neutral words and its Recovered files folder (the backend's trash_dir);
// its secrets are a private file in MiniClaw's data folder (server/src/secrets.rs).
export const words = {
  get windows() { return host.os === 'windows'; },
  get linux() { return host.os === 'linux'; },
  get mac() { return host.os === 'macos'; },
  get machine() { return this.mac ? 'this Mac' : 'this computer'; },
  get matchSystem() { return this.mac ? 'Match macOS' : 'Match system'; },
  get secretStore() {
    return this.windows ? 'Windows Credential Manager' : this.linux ? "MiniClaw's private data folder" : 'the macOS Keychain';
  },
  get secretStoreShort() {
    return this.windows ? 'Windows Credential Manager' : this.linux ? "MiniClaw's private data folder" : 'the Keychain';
  },
  get account() { return this.mac ? 'your macOS user' : 'your user account'; },
  get recovery() {
    return this.mac ? 'the Trash on this Mac, or Transfers/discarded in a VM' : "MiniClaw's Recovered files folder";
  },
  // Where a deleted conversation's own folder goes when the user asks.
  get folderTrash() { return this.mac ? 'the Trash' : "MiniClaw's Recovered files folder"; },
  get commandPlaceholder() {
    return this.windows ? 'Absolute executable path, or npx' : this.linux ? '/usr/bin/npx' : '/opt/homebrew/bin/npx';
  },
  get folderPlaceholder() {
    return this.windows ? 'Absolute working folder' : this.linux ? '/home/you/mcp/notes' : '/Users/you/mcp/notes';
  },
  get workspacePlaceholder() {
    return this.windows ? 'Absolute project folder' : this.linux ? '/home/you/Projects/site' : '/Users/you/Projects/site';
  },
  // AppleScript rules exist only on macOS.
  get appleScript() { return this.mac; },
};
