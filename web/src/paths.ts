// Splits file paths for display. Windows paths use either separator; on macOS a backslash
// is an ordinary filename character and must stay in the name.
import { host } from './platform.svelte';

// Recognises a drive or share path whatever the platform, and any path on a Windows backend.
function windowsPath(path: string) {
  return host.os === 'windows' || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith('\\\\');
}

// Returns the path's parts and the separator it is written with.
function parts(path: string) {
  if (!windowsPath(path)) return { names: path.split('/'), separator: '/' };
  return { names: path.split(/[\\/]/), separator: path.includes('\\') ? '\\' : '/' };
}

// Returns the last part of a path: its file or folder name.
export function fileName(path: string) {
  return parts(path).names.pop() ?? '';
}

// Returns the last `count` parts of a path, joined as the path itself is written.
export function lastParts(path: string, count: number) {
  const { names, separator } = parts(path);
  return names.slice(-count).join(separator);
}
