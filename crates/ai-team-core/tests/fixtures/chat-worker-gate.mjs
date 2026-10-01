import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
const root = process.env.BUILD_TEST_ROOT;
if (process.env.ANTHROPIC_API_KEY || process.env.OPENAI_API_KEY) throw Error('metered credentials leaked into a gate');
const mode = fs.readFileSync(path.join(root, 'worker-mode'), 'utf8').trim();
fs.appendFileSync(path.join(root, 'gate-calls'), process.cwd() + '\n');
if (mode === 'gate-edit') fs.appendFileSync('crates/S1.txt', 'unverified gate edit\n');
if (mode === 'slow-gate' || mode === 'orphan-gate') {
  const child = spawn('sh', ['-c', 'trap "" TERM; while :; do sleep 1; done'], { stdio: 'inherit' });
  fs.writeFileSync(path.join(root, 'gate-tool.pid'), String(child.pid));
  fs.writeFileSync(path.join(root, 'gate-waiting'), 'waiting');
  if (mode === 'slow-gate') await new Promise(() => { setInterval(() => {}, 1000); });
  console.log('gate finished; helper is not allowed to outlive it');
  process.exit(0);
}
for (const dir of ['crates', 'ui']) {
  if (!fs.existsSync(dir)) continue;
  for (const name of fs.readdirSync(dir).filter(name => /^S\d+\.txt$/.test(name))) {
    if (fs.readFileSync(`${dir}/${name}`, 'utf8') !== 'GOOD\n') {
      console.error('implementation is BAD: ' + name);
      process.exitCode = 1;
    }
  }
}
// Generated/untracked output must not be silently added to the source candidate.
fs.mkdirSync('dist', { recursive: true });
fs.writeFileSync('dist/generated.txt', 'not maker work');
