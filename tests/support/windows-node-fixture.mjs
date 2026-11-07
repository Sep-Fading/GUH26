// Stands in for the private Node files of a Windows bundle, without the official archive.
import { sandboxNodeNames } from '../../scripts/setup/windows-node-sandbox.mjs';

// Returns one placeholder per private Node file.
export const nodeFixture = async () => new Map(sandboxNodeNames.map(name => [name, Buffer.from(name)]));
