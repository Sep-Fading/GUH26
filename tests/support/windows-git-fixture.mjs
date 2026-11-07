// Stands in for the bundled Git runtime when a test assembles a Windows package.
import { windowsGitBundle } from '../../scripts/setup/windows-git-bundle.mjs';

// Returns the files gitBundleFiles would supply: the archive and its licence texts.
export async function gitFixture({ arch }) {
  return new Map([[windowsGitBundle(arch).file, Buffer.from(`git ${arch}`)],
    ['licenses/git-COPYING.txt', Buffer.from('GPL')], ['licenses/NOTICE.txt', Buffer.from('notice')],
    ['licenses/SOURCE-OFFER.txt', Buffer.from('offer')]]);
}
