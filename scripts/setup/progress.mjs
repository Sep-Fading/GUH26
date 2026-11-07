// Returns a fixed-width byte bar readable without color or Unicode support.
export function progressBar(received, total) {
  const ratio = Math.min(1, Math.max(0, received / total));
  const filled = Math.floor(ratio * 24);
  return `[${'#'.repeat(filled)}${'-'.repeat(24 - filled)}] ${Math.floor(ratio * 100)}%`;
}
