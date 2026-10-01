// Fake pool transport, actual Git worktrees. Everything lives under BUILD_TEST_ROOT.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
const root = process.env.BUILD_TEST_ROOT;
const args = process.argv.slice(2);
if (args[0] === 'status' && fs.readFileSync(path.join(root, 'worker-mode'), 'utf8').trim() === 'slow-status') {
  fs.writeFileSync(path.join(root, 'status-waiting.pid'), String(process.pid));
  await new Promise(() => { setInterval(() => {}, 1000); });
}
const lock = path.join(root, 'pool-lock');
let acquired = false;
for (let i = 0; i < 1000; i++) {
  try { fs.mkdirSync(lock); acquired = true; break; } catch (e) { if (e.code !== 'EEXIST') throw e; }
  await new Promise(r => setTimeout(r, 10));
}
if (!acquired) throw Error('fixture pool lock timed out');
try {
  const state = path.join(root, 'pool.json');
  const entries = fs.existsSync(state) ? JSON.parse(fs.readFileSync(state)) : [];
  const git = (cwd, ...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  fs.appendFileSync(path.join(root, 'pool-calls'), JSON.stringify(args) + '\n');
  if (args[0] === 'get') {
    const lease = path.join(root, 'worker-lease-' + (entries.length + 1));
    git(process.cwd(), 'worktree', 'add', '--detach', lease, 'HEAD');
    entries.push({ name: path.basename(lease), path: lease, status: 'leased', leaseHolder: args[args.indexOf('--lease-holder') + 1], processes: [] });
    fs.writeFileSync(state, JSON.stringify(entries));
    if (fs.readFileSync(path.join(root, 'worker-mode'), 'utf8').trim() === 'acquire-fail') throw Error('injected acquisition failure after leasing');
    console.log(lease);
  } else if (args[0] === 'status') {
    console.log(JSON.stringify({ worktrees: entries }));
  } else if (args[0] === 'return') {
    const entry = entries.find(e => fs.realpathSync(e.path) === fs.realpathSync(args[1]));
    if (!entry || entry.status !== 'leased') throw Error('wrong return target');
    if (git(entry.path, 'status', '--porcelain', '--untracked-files=no')) throw Error('verified lease has a stale or dirty real index');
    if (fs.readFileSync(path.join(root, 'worker-mode'), 'utf8').trim() === 'return-fail') throw Error('injected awt return failure');
    git(entry.path, 'checkout', '--detach', '--force', git(process.cwd(), 'rev-parse', 'HEAD'));
    git(entry.path, 'clean', '-fd');
    entry.status = 'available'; entry.leaseHolder = null;
    fs.writeFileSync(state, JSON.stringify(entries));
  } else { throw Error('unexpected awt operation: ' + args); }
} finally { fs.rmdirSync(lock); }
