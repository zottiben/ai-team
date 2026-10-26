// HTTP lifecycle fixture: real processes and Git, no model/network or operator state.
import fs from 'node:fs';
import path from 'node:path';
const root = process.env.BUILD_TEST_ROOT;
const args = process.argv.slice(2);
const value = flag => args[args.indexOf(flag) + 1];
if (!args.includes('--mcp-config')) throw Error('unexpected Pi metadata/auth invocation');
if (process.env.ANTHROPIC_API_KEY || process.env.OPENAI_API_KEY || process.env.PI_MCP_CONFIG_MODE !== 'exclusive') throw Error('wrong environment');
const config = JSON.parse(fs.readFileSync(value('--mcp-config')));
if (args.includes('--list-models')) {
  if (Object.keys(config.mcpServers).length) throw Error('catalogue inherited MCP servers');
  console.log('provider model context max-out thinking images\nllama.cpp fixture 200k 32k yes yes\nllama.cpp alternate 32k 8k yes yes');
  process.exit(0);
}
if (value('--mode') === 'rpc') {
  if (Object.keys(config.mcpServers).length || !args.includes('--no-session') || !args.includes('--no-mcp') || !args.includes('--no-tools') || args.includes('--thinking') || !process.env.AI_TEAM_THINKING_PROBE) throw Error('unsafe thinking probe');
  console.log(JSON.stringify({type:'ai_team_thinking', nonce:process.env.AI_TEAM_THINKING_PROBE, provider:value('--provider'), model:value('--model'), levels:['off','minimal','low','medium','high','xhigh','max']}));
  process.exit(0);
}
const server = config.mcpServers['ai-team-planner'];
const node = server.args[server.args.indexOf('--node') + 1];
const chat = server.args[server.args.indexOf('--chat') + 1];
const prompt = args.at(-1);
const role = prompt.match(/You are the (\S+) seat/)?.[1] ?? 'assistant';
const key = prompt.match(/## Assigned slice ([^:]+):/)?.[1];
const session = args.includes('--session-id') ? value('--session-id') : `http-session-${node}`;
const mode = fs.readFileSync(path.join(root, 'worker-mode'), 'utf8').trim();
const images = args.filter(arg => arg.startsWith('@')).map(arg => {
  const file = arg.slice(1);
  if (file.startsWith(process.cwd() + path.sep)) throw Error('image was written in checkout');
  if ((fs.statSync(file).mode & 0o777) !== 0o600 || (fs.statSync(path.dirname(file)).mode & 0o777) !== 0o700) throw Error('image is not private');
  return { file, data: fs.readFileSync(file).toString('base64') };
});
fs.appendFileSync(path.join(root, 'pi-calls'), JSON.stringify({ chat, node, role, key, session, cwd:process.cwd(), prompt, images, provider:value('--provider'), model:value('--model'), thinking:value('--thinking'), requested_thinking:process.env.AI_TEAM_THINKING }) + '\n');
const emit = event => console.log(JSON.stringify(event));
emit({type:'session',id:session,cwd:process.cwd()});
if (mode !== 'no-echo') emit({type:'message_end',message:{role:'user',content:[{type:'text',text:images.map(image => `<file name="${image.file}"></file>\n`).join('') + prompt}, ...images.map(image => ({type:'image',mimeType:'image/png',data:image.data}))]}});
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
emit({type:'turn_end',message:{role:'assistant',stopReason:mode === 'fail-solo' ? 'error' : 'stop',usage:{input:20,output:10}}});
emit({type:'agent_settled'});
