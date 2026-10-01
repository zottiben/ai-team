// Deterministic in-process model only. Pi, tool execution, the guard, MCP, and sessions are real.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import { createAssistantMessageEventStream, getCurrentTools } from '@earendil-works/pi-ai/compat';
const root = process.env.BUILD_TEST_ROOT;
const cost = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 };
const contextFailure = fs.existsSync(root + '/context-failure');
let script, pending, snapshot, serial = 0;
const audit = value => fs.appendFileSync(`${root}/model-${process.pid}.jsonl`, JSON.stringify({ time: Date.now(), pid: process.pid, contextFailure, ...value }) + '\n');
const call = (name, args, error = false) => ({ name, args, error });
const plan = (name, args = {}, error = false) => ({ name, args, error, planner: true });

function* steps(role) {
  if (role === 'probe') {
    yield call('mcp', { server: 'unscoped-sentinel' });
    return 'DISCOVERY_CONTROL';
  }
  if (contextFailure && role === 'orchestrator') {
    yield { ...call('mcp', { connect: 'clickup' }), contextError: true };
    return 'Everything is grounded; proceed to planning.';
  }
  yield call('mcp', { connect: 'ai-team-planner' });
  yield plan('get_plan');
  if (role === 'solo') {
    yield plan('get_plan', { chat_id: 2 }, 'get_plan accepts no scope overrides');
    if (!snapshot.bundle) {
      yield plan('create_plan', { title: 'Transport-owned chat' });
      yield plan('open_question', { body: 'Keep the source checkout unchanged?' });
    }
    yield plan('append_log', { body: 'SOLO_MCP_SENTINEL' });
    return 'SOLO_CONTEXT_SENTINEL';
  }
  if (role === 'orchestrator') {
    yield plan('append_log', { body: 'COORDINATOR_MCP_SENTINEL' });
    return 'COORDINATOR_CONTEXT_SENTINEL: ask the planner to implement S1 in crates/answer.txt.';
  }
  if (role === 'planner') {
    yield plan('add_slice', { key: 'S1', title: 'Real Pi writes the answer', scope: 'Write GOOD followed by a newline.', touches: ['crates/answer.txt'], demo: 'npm test and independent reader' });
    return 'PLANNER_CONTEXT_SENTINEL';
  }
  if (role === 'backend') {
    yield call('write', { path: root + '/outside.txt', content: 'WRONG' }, 'outside the worktree');
    yield call('bash', { command: 'git push' }, 'publish');
    yield call('bash', { command: 'aip show' }, 'Use its scoped ai-team-planner MCP tools');
    yield plan('set_slice_status', { key: 'S1', status: 'done' }, 'completion and claim release belong to the controller');
    yield call('write', { path: 'crates/answer.txt', content: 'GOOD\n' });
    yield plan('append_log', { body: 'MAKER_MCP_SENTINEL', slice: 'S1' });
    return 'MAKER_CONTEXT_SENTINEL: implemented S1, not committed.';
  }
  assert.equal(role, 'verifier');
  yield call('write', { path: 'crates/answer.txt', content: 'WRONG' }, 'Tool write not found');
  yield call('read', { path: 'crates/answer.txt' });
  return 'VERIFIER_CONTEXT_SENTINEL\nVERDICT: pass';
}

function reply(model, context) {
  const role = model.id.replace('fixture-', '');
  const tools = getCurrentTools(context.messages).map(t => t.name);
  audit({ role, activeTools: tools });
  assert(!tools.some(n => n.includes('ai-planner') || n.includes('delete_plan') || n.includes('answer_question')));
  if (role !== 'probe') {
    assert(!tools.some(n => n.includes('unscoped-sentinel')));
    assert.equal(process.env.AI_TEAM_PLAN, 'embedded');
  }
  if (role !== 'solo' && role !== 'backend' && role !== 'probe') assert(!tools.includes('write') && !tools.includes('edit'));
  const planning = tools.filter(n => n.startsWith('ai-team-planner_')).map(n => n.slice('ai-team-planner_'.length));
  const allowed = {
    orchestrator: ['get_plan', 'open_question', 'append_log'],
    backend: ['get_plan', 'set_slice_status', 'open_question', 'append_log'],
    verifier: ['get_plan'],
  }[role];
  if (allowed) assert(planning.every(n => allowed.includes(n)), `${role} discovered excess authority: ${planning}`);
  if (!script) {
    audit({ role, tools, messages: context.messages });
    script = steps(role);
  }
  if (pending) {
    const result = context.messages.findLast(m => m.role === 'toolResult');
    audit({ role, result });
    assert.equal(result?.toolName, pending.name);
    if (pending.contextError) {
      assert.equal(result.details.server, 'clickup');
      assert.equal(result.details.error, 'not_found');
    } else if (role !== 'probe') assert.equal(Boolean(result.isError), Boolean(pending.error), JSON.stringify(result));
    if (pending.error) assert(result.content.some(c => c.type === 'text' && c.text.includes(pending.error)), JSON.stringify(result));
    if (pending.planner && !pending.error) {
      const text = result.content.filter(c => c.type === 'text').map(c => c.text).join('\n');
      snapshot = JSON.parse(text);
      assert.equal(snapshot.chat_id, 1, 'MCP crossed chat scope');
      assert(Number.isInteger(snapshot.revision), text);
    }
  }
  const next = script.next();
  if (next.done) return { type: 'text', text: next.value };
  pending = next.value;
  if (pending.planner) {
    const name = tools.find(n => n.endsWith('_' + pending.name));
    assert(name, `Missing ${pending.name}; tools: ${tools}`);
    pending.name = name;
    if (!name.endsWith('_get_plan')) pending.args = { ...pending.args, expect_revision: snapshot.revision };
  }
  return { type: 'toolCall', id: `fixture-${++serial}`, name: pending.name, arguments: pending.args };
}

export default function (pi) {
  audit({ factory: true });
  assert.equal(process.env.ANTHROPIC_API_KEY, undefined);
  assert.equal(process.env.OPENAI_API_KEY, undefined);
  const models = ['solo', 'orchestrator', 'planner', 'backend', 'verifier', 'probe'].map(role => ({
    id: 'fixture-' + role, name: 'Offline ' + role, reasoning: true, input: ['text'],
    api: 'openai-completions', provider: 'llama.cpp', baseUrl: 'http://127.0.0.1:9',
    contextWindow: 200000, maxTokens: 4096, cost,
  }));
  const streamSimple = (model, context, options) => {
      const stream = createAssistantMessageEventStream();
      const message = { role: 'assistant', content: [], api: model.api, provider: model.provider, model: model.id,
        usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost }, stopReason: 'pending', timestamp: Date.now() };
      queueMicrotask(async () => {
        try {
          assert(!options?.signal?.aborted, 'aborted fixture request');
          const block = reply(model, context);
          if (model.id === 'fixture-backend' && block.type === 'text' && fs.existsSync(root + '/pause-maker')) {
            fs.writeFileSync(root + '/maker-paused', String(process.pid));
            await new Promise((_, reject) => options.signal.addEventListener('abort', () => reject(new Error('fixture aborted')), { once: true }));
          }
          stream.push({ type: 'start', partial: message });
          message.content.push(block);
          if (block.type === 'text') {
            stream.push({ type: 'text_start', contentIndex: 0, partial: message });
            stream.push({ type: 'text_delta', contentIndex: 0, delta: block.text, partial: message });
            stream.push({ type: 'text_end', contentIndex: 0, content: block.text, partial: message });
            message.stopReason = 'stop';
          } else {
            stream.push({ type: 'toolcall_start', contentIndex: 0, partial: message });
            stream.push({ type: 'toolcall_delta', contentIndex: 0, delta: JSON.stringify(block.arguments), partial: message });
            stream.push({ type: 'toolcall_end', contentIndex: 0, toolCall: block, partial: message });
            message.stopReason = 'toolUse';
          }
          stream.push({ type: 'done', reason: message.stopReason, message });
        } catch (error) {
          const reason = options?.signal?.aborted ? 'aborted' : 'error';
          audit(reason === 'error' ? { failure: String(error) } : { aborted: true });
          message.stopReason = reason; message.errorMessage = String(error);
          stream.push({ type: 'error', reason, error: message });
        } finally { stream.end(); }
      });
      return stream;
  };
  const provider = {
    id: 'llama.cpp', name: 'Offline transport fixture', getModels: () => models,
    auth: { apiKey: {
      name: 'Offline fixture',
      check: async () => ({ type: 'api_key', source: 'offline fixture' }),
      resolve: async () => ({ auth: { apiKey: 'not-a-credential' }, source: 'offline fixture' }),
    } },
    stream: streamSimple, streamSimple,
  };
  pi.registerProvider(provider);
  pi.on('session_start', () => { audit({ session: true }); pi.registerProvider(provider); });
}
