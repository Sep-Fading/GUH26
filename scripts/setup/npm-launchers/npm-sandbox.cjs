// MiniClaw: loaded by the npm and npx launchers inside the sandbox only. npm's dependency tree
// code checks every folder of a path, from the drive root down, for links (arborist realpath),
// except below its working folder. The sandbox may not look at folders above the ones the run
// was granted (C:\Users and the like). For a folder above the working folder, npm's cache or
// its global folder, a refused check is answered as "a folder, not a link"; nothing else changes.
'use strict';
const fs = require('node:fs');
const path = require('node:path');

const roots = [process.cwd(), process.env.npm_config_cache, process.env.LOCALAPPDATA, process.env.npm_config_prefix]
  .filter(Boolean).map(root => path.resolve(root).toLowerCase());

// Whether a path is a folder above one of the run's own folders.
function above(target) {
  const folder = path.resolve(String(target)).toLowerCase().replace(/[\\/]+$/, '') + path.sep;
  return roots.some(root => root.startsWith(folder));
}

// A refused check of such a folder, answered as a plain folder.
function folder(error, target) {
  if (!error || !['EPERM', 'EACCES'].includes(error.code) || !above(target)) return null;
  return Object.assign(Object.create(fs.Stats.prototype), { mode: fs.constants.S_IFDIR | 0o755, nlink: 1, size: 0 });
}

const lstatPromise = fs.promises.lstat;
fs.promises.lstat = async function lstat(target, ...rest) {
  try { return await lstatPromise.call(this, target, ...rest); } catch (error) { const stats = folder(error, target); if (stats) return stats; throw error; }
};
const lstatSync = fs.lstatSync;
fs.lstatSync = function lstat(target, ...rest) {
  try { return lstatSync.call(this, target, ...rest); } catch (error) { const stats = folder(error, target); if (stats) return stats; throw error; }
};
const lstatCallback = fs.lstat;
fs.lstat = function lstat(target, ...rest) {
  const done = rest.pop();
  lstatCallback.call(this, target, ...rest, (error, stats) => { const answer = folder(error, target); answer ? done(null, answer) : done(error, stats); });
};
