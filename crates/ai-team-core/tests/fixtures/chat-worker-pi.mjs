// Actual offline child turns. Validate generated bindings, never contact any model.
import fs from 'node:fs';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
const root = process.env.BUILD_TEST_ROOT;
const args = process.argv.slice(2);
const value = flag => args[args.indexOf(flag) + 1];
const prompt = args.at(-1);
const role = prompt.match(/You are the (\S+) seat/)?.[1] ?? 'assistant';
const key = prompt.match(/## Assigned slice ([^:]+):/)?.[1] ?? 'solo';
const configPath = value('--mcp-config');
const config = JSON.parse(fs.readFileSync(configPath));
const server = config.mcpServers['ai-team-planner'];
const id = server.args[server.args.indexOf('--node') + 1];
const session = args.includes('--session-id') ? value('--session-id') : `worker-session-${id}`;
const excluded = value('--exclude-tools').split(',');
if (process.env.ANTHROPIC_API_KEY || process.env.OPENAI_API_KEY || process.env.PI_MCP_CONFIG_MODE !== 'exclusive') throw Error('wrong environment');
if (config.mcpServers['ai-planner'] || server.args[server.args.indexOf('--chat') + 1] !== '1') throw Error('wrong embedded scope');
if (path.dirname(configPath).startsWith(process.cwd())) throw Error('support is writable by the seat');
if (role === 'verifier') {
  if (!excluded.includes('write') || !excluded.includes('edit') || server.includeTools.join(',') !== 'get_plan') throw Error('reader permissions widened');
} else if (role !== 'assistant' && (excluded.includes('write') || server.includeTools.includes('add_slice'))) throw Error('maker permissions wrong');
fs.writeFileSync(path.join(root, `worker-${id}.prompt`), prompt);
fs.appendFileSync(path.join(root, 'worker-calls'), JSON.stringify({ role, key, id, session, configPath, cwd: process.cwd(), pid: process.pid }) + '\n');
const emit = event => console.log(JSON.stringify(event));
emit({type:'session', id:session, cwd:process.cwd()});
emit({type:'message_end', message:{role:'user', content:[{type:'text', text:'VERDICT: pass'}]}});
const mode = fs.readFileSync(path.join(root, 'worker-mode'), 'utf8').trim();
if (role !== 'verifier') {
  const dir = role === 'frontend' ? 'ui' : 'crates';
  fs.mkdirSync(dir, { recursive: true });
  if (mode !== 'noop') fs.writeFileSync(`${dir}/${key}.txt`, (mode === 'repair' && !args.includes('--session-id')) || (mode === 'siblings' && key === 'S1') ? 'BAD\n' : 'GOOD\n');
  if (mode === 'outside') fs.writeFileSync('outside.txt', 'not approved');
  if (mode === 'staged-only') {
    fs.writeFileSync('crates/staged-only.txt', 'preserve staged-only work\n');
    execFileSync('git', ['add', 'crates/staged-only.txt']);
    fs.unlinkSync('crates/staged-only.txt');
  }
  if (mode === 'tracked-output') fs.renameSync('crates/dist/base.txt', 'crates/dist/new.txt');
  if (mode === 'rename') fs.renameSync('crates/old name-é.txt', 'crates/new name-é.txt');
  if (mode === 'slow-maker' || mode === 'settled-maker') {
    if (mode === 'settled-maker') emit({type:'agent_settled'});
    const child = spawn('sh', ['-c', 'trap "" TERM; while :; do sleep 1; done'], { stdio: 'ignore' });
    fs.writeFileSync(path.join(root, 'worker-tool.pid'), String(child.pid));
    fs.writeFileSync(path.join(root, 'worker-waiting'), id);
    await new Promise(() => { setInterval(() => {}, 1000); });
  }
}
await new Promise(r => setTimeout(r, key === 'S1' ? 300 : 160));
if (role === 'verifier' && mode === 'verifier-edit') fs.appendFileSync(`crates/${key}.txt`, 'unverified change\n');
if (!(role === 'verifier' && mode === 'echo-only')) emit({type:'message_end', message:{role:'assistant', content:[{type:'text', text:role === 'verifier' ? (mode === 'reject' ? 'Missing wiring.\nVERDICT: reject' : 'Existence, substance and wiring checked.\nVERDICT: pass') : `Implemented ${key}.`} ]}});
emit({type:'turn_end', message:{role:'assistant', stopReason:'stop', usage:{input:20, output:10}}});
emit({type:'agent_settled'});
if (['exit-fail', 'settle-fail'].includes(mode) && role !== 'verifier') process.exitCode = 7;
