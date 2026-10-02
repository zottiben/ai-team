// HTTP lifecycle fixture: real processes and Git, no model/network or operator state.
import fs from 'node:fs';
import path from 'node:path';
const root = process.env.BUILD_TEST_ROOT;
const args = process.argv.slice(2);
const value = flag => args[args.indexOf(flag) + 1];
if (!args.includes('--mcp-config')) throw Error('unexpected Pi metadata/auth invocation');
if (process.env.ANTHROPIC_API_KEY || process.env.OPENAI_API_KEY || process.env.PI_MCP_CONFIG_MODE !== 'exclusive') throw Error('wrong environment');
const config = JSON.parse(fs.readFileSync(value('--mcp-config')));
const server = config.mcpServers['ai-team-planner'];
const node = server.args[server.args.indexOf('--node') + 1];
const chat = server.args[server.args.indexOf('--chat') + 1];
const prompt = args.at(-1);
const role = prompt.match(/You are the (\S+) seat/)?.[1] ?? 'assistant';
const key = prompt.match(/## Assigned slice ([^:]+):/)?.[1];
const session = args.includes('--session-id') ? value('--session-id') : `http-session-${node}`;
const mode = fs.readFileSync(path.join(root, 'worker-mode'), 'utf8').trim();
fs.appendFileSync(path.join(root, 'pi-calls'), JSON.stringify({ chat, node, role, key, session, cwd:process.cwd(), prompt }) + '\n');
const emit = event => console.log(JSON.stringify(event));
emit({type:'session',id:session,cwd:process.cwd()});
if (key && role !== 'verifier') {
  fs.mkdirSync('crates', {recursive:true});
  fs.writeFileSync(`crates/${key}.txt`, `Built ${key}\n`);
}
if ((role === 'assistant' && mode === 'hold-solo') || (key && role !== 'verifier' && mode === 'slow-maker')) {
  fs.writeFileSync(path.join(root, `waiting-${node}`), String(process.pid));
  emit({type:'message_update',assistantMessageEvent:{type:'text_delta',contentIndex:0,delta:`Active ${role}`}});
  while (!fs.existsSync(path.join(root, `release-${node}`))) await new Promise(r => setTimeout(r, 20));
}
emit({type:'message_end',message:{role:'assistant',content:[{type:'text',text:role === 'verifier' ? (mode === 'reject' ? 'VERDICT: reject' : 'VERDICT: pass') : `Finished ${role}.`} ]}});
emit({type:'turn_end',message:{role:'assistant',stopReason:'stop',usage:{input:20,output:10}}});
emit({type:'agent_settled'});
