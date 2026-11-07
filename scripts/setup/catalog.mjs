// Shares byte units with installation sizing.
import { GiB } from './plan.mjs';

// Pinned images tested in the lab; updates require new vendor size/hash verification.
export const images = [
  { id: 'linux', name: 'Debian 13 ARM64 (20260831-2587)',
    file: 'debian-13-generic-arm64-20260831-2587.tar.xz',
    url: 'https://cloud.debian.org/images/cloud/trixie/20260831-2587/debian-13-generic-arm64-20260831-2587.tar.xz',
    bytes: 303762952, expandedBytes: 3 * GiB, algorithm: 'sha512',
    digest: Buffer.from('10HuS994QnRG9H0zCyg8wYywT17m7AA6FfZAXHQPQYFdnlVZK4bJ6I6DLXCSKKbG6mdkwThPj3mH1If/MN7FEw', 'base64').toString('hex') },
  { id: 'macos', name: 'macOS 26.6.2 (25G83)',
    file: 'UniversalMac_26.6.2_25G83_Restore.ipsw',
    url: 'https://updates.cdn-apple.com/2026SummerFCS/fullrestores/140-75212/A2A24B94-1FC1-45A3-93F7-C51B02AF1F4D/UniversalMac_26.6.2_25G83_Restore.ipsw',
    bytes: 19772231540, expandedBytes: 0, algorithm: 'sha256',
    digest: '885503b7f4b06609e9a512f2befd40f59730640a3f1233e3892d60affdd51c95' },
];
