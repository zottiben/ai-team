const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const assert = (ok, message) => { if (!ok) throw Error(message); };
async function until(test, label) { for (let i = 0; i < 160; i++) { if (test()) return; await pause(30); } throw Error(label); }
const fixture = () => import([...document.scripts].find(script => new URL(script.src, location.href).pathname === '/src/synth/shell-preview.tsx').src);

export function inspectActiveOverview() {
  const pane = document.querySelector('#chat-pane-overview'), conversation = document.querySelector('.chat-conversation');
  assert(pane?.checkVisibility(), 'Selected chat did not open Overview automatically');
  assert(conversation?.checkVisibility(), 'Overview hid the conversation');
  const composer = document.querySelector('.chat-composer'), tracker = document.querySelector('.chat-transcript-shell');
  assert(composer?.checkVisibility() && tracker?.checkVisibility(), 'Overview lost tracker or composer');
  const patch = document.querySelector('.synth-patch-bay');
  if (patch) {
    assert(conversation.contains(patch), 'Team patch bay is not on the conversation side');
    for (const selector of ['.synth-overview-rack', '.synth-patch-inspector', '.synth-repository-view']) {
      assert(pane.contains(document.querySelector(selector)), `${selector} escaped the right Overview sidebar`);
    }
    assert(!conversation.querySelector('.synth-overview-rack, .synth-patch-inspector'), 'Team telemetry was duplicated on the left');
    const body = document.querySelector('.chat-body');
    if (body.dataset.synthStacked === 'false') {
      const side = document.querySelector('.chat-context').getBoundingClientRect(), left = conversation.getBoundingClientRect(), prompt = composer.getBoundingClientRect(), log = tracker.getBoundingClientRect();
      assert(side.left >= left.right - 1, 'Overview sidebar overlaps the conversation');
      assert(prompt.bottom <= innerHeight + 1 && prompt.top >= log.bottom - 1, 'Team composer is not kept visible below its tracker');
      assert(log.height >= 170, 'Team tracker lost its usable height');
    }
  }
  assert(document.documentElement.scrollWidth <= innerWidth + 1, 'Overview overflowed the page');
  return { automaticOverview: true, conversationVisible: true, teamSidebar: !!patch };
}

export async function exerciseActiveOverview() {
  await until(() => document.querySelector('.synth-repository-view') && document.querySelectorAll('.synth-graph[data-active]').length >= 8, 'Overview did not load');
  await document.fonts.ready; await pause(200);
  return { ...inspectActiveOverview(), ...await exerciseScopeLiveness() };
}

export async function exerciseScopeLiveness() {
  const source = await fixture(), scopes = () => [...document.querySelectorAll('.synth-graph[data-active]')];
  await until(() => scopes().length >= 8, 'Live diagnostic scopes unavailable');
  const graph = scopes()[0], phase = () => graph.style.backgroundPositionX;
  graph.scrollIntoView({ block: 'center' });
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  for (const state of ['idle', 'running', 'stopping', 'awaiting_approval', 'running', 'failed', 'interrupted']) {
    source.setScopeState(state, state === 'awaiting_approval');
    const active = state === 'running' || state === 'stopping';
    await until(() => scopes().every(scope => scope.dataset.active === String(active)), `Scopes kept the wrong activity for ${state}`);
    await pause(180);
    const before = phase(); await pause(220);
    assert(active && !reduced ? phase() !== before : phase() === before, `Scope motion disagrees with ${state} / reduced=${reduced}`);
  }
  const owner = graph;
  if (!new URLSearchParams(location.search).has('team-overview')) {
    source.advanceOverviewTelemetry();
    await until(() => document.querySelector('.synth-token-scope').textContent.includes('950 cached tokens'), 'Late real cache receipt was lost after stop');
    assert(graph === owner && graph.dataset.active === 'false', 'Late receipt restarted or replaced a stopped scope');
    const before = phase(); await pause(220); assert(phase() === before, 'Late receipt restarted display motion');
  }
  // Every channel can be revealed without reactivating a stopped chat.
  for (const scope of scopes().filter(scope => !scope.closest('[hidden]'))) {
    scope.scrollIntoView({ block: 'center' }); await pause(120);
    const before = scope.style.backgroundPositionX; await pause(120);
    assert(scope.style.backgroundPositionX === before, 'Revealing a recorded scope restarted its clock');
  }
  assert(source.fixtureCommands.length === 0, 'Scope presentation dispatched a command');
  return { scopes: scopes().length, stoppedStates: 4, retainedFinalReceipt: true, commands: 0 };
}
