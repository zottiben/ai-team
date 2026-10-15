// Real planning controller + explicitly enabled, in-memory status service. Not native input acceptance.
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const must = (value, message) => { if (!value) throw Error(message); return value; };
async function until(check, message) { const end = performance.now() + 5000; while (performance.now() < end) { const value = check(); if (value) return value; await delay(20); } throw Error(message); }
const key = (value, code = value) => document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: value, code, bubbles: true, cancelable: true }));
async function setup() {
  must(!document.hidden, 'Native viewport is hidden; production Board input verification blocked');
  must(new URLSearchParams(location.search).get('board-input') === 'true', 'Enable only the synthetic Board status service');
  const tab = await until(() => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.trim() === 'Board'), 'Chat Board tab missing'); tab.click();
  const board = await until(() => document.querySelector('.plan-kanban'), 'Plan board missing');
  board.scrollIntoView({ block: 'start' });
  await until(() => board.dataset.statusDrag === 'ready', 'Status adapter did not become ready');
  const entry = [...document.scripts].find(script => new URL(script.src || location.href).pathname.endsWith('/src/synth/shell-preview.tsx'));
  const fixture = await import(must(entry, 'Exact shell entry missing').src);
  must(board.querySelectorAll('[data-slice-key]').length === 100 && document.querySelectorAll('.plan-kanban').length === 1, 'Wrong fixture cardinality');
  return { board, fixture, panel: board.closest('.chat-planning') };
}
let pickups = 0;
async function start(board, id = 'S1') {
  const attempt = ++pickups;
  const handle = must(board.querySelector(`[data-slice-drag="${id}"]`), 'Move handle missing');
  handle.scrollIntoView({ block: 'center', inline: 'center' });
  await until(() => !handle.disabled && board.dataset.statusDrag === 'ready', 'Move handle did not become available');
  handle.focus(); key(' ', 'Space');
  await until(() => board.dataset.dragging === 'true', `Production keyboard pickup ${attempt} failed`);
  return handle;
}
async function lane(board, direction, target) { key(direction); await until(() => board.querySelector(`[data-status="${target}"][data-over="true"]`), `Keyboard target ${target} missing`); await delay(30); }
async function drop(board) { key(' ', 'Space'); await until(() => !board.dataset.dragging, 'Drop did not settle'); }
const field = form => form.querySelector('input');
function type(input, value) { input.focus(); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, value); input.dispatchEvent(new Event('input', { bubbles: true })); }
export async function exerciseProductionBoard() {
  const { board, fixture, panel } = await setup();
  await start(board); await lane(board, 'ArrowRight', 'active'); await drop(board);
  await until(() => fixture.boardState().snapshot.bundle.slices[0].status === 'active', 'Status-only service did not record the move');
  const moved = await until(() => board.querySelector('[data-status="active"] [data-slice-drag="S1"]'), 'Authoritative card did not move');
  await until(() => document.activeElement === moved, 'Accepted keyboard move lost logical card focus');
  must(board.querySelector('[data-status="active"] [data-slice-key]').dataset.sliceKey === 'S1', 'Move invented a ranking');
  let requests = fixture.boardState().requests;
  must(requests.length === 1 && requests[0].action === 'set_slice_status' && requests[0].expect_revision === 4 && Object.keys(requests[0]).sort().join(',') === 'action,expect_revision,key,status', 'Move changed another contract');
  await start(board); key('ArrowDown'); await delay(50); await drop(board);
  must(fixture.boardState().requests.length === 1, 'Within-column arrow invented a mutation');
  await start(board); await lane(board, 'ArrowRight', 'in_review'); key('Escape');
  await until(() => !board.dataset.dragging, 'Escape did not cancel');
  await start(board); key('Tab'); await until(() => !board.dataset.dragging, 'Tab did not cancel');
  await start(board); await lane(board, 'ArrowRight', 'in_review'); await lane(board, 'ArrowRight', 'blocked'); await drop(board);
  const form = await until(() => panel.querySelector('.synth-blocked-drop'), 'Blocked drop did not ask for its reason');
  const input = field(form); await until(() => document.activeElement === input, 'Blocked drop did not focus its reason');
  input.scrollIntoView({ block: 'end' }); await delay(150);
  must(!input.readOnly, 'Visible blocker draft became read-only when its Board scrolled offscreen');
  must(fixture.boardState().requests.length === 1 && fixture.boardState().snapshot.bundle.slices[0].status === 'active', 'Blocked drop wrote before a reason');
  type(input, 'Synthetic blocking decision'); await delay(20);
  form.querySelector('button[type="button"]').click();
  await until(() => !panel.querySelector('.synth-blocked-drop'), 'Cancelling a blocker left its proposal open');
  must(fixture.boardState().requests.length === 1, 'Cancelling a blocker changed the plan');
  await until(() => document.activeElement?.dataset.sliceDrag === 'S1', 'Blocker cancellation lost its logical handle focus');
  await start(board); fixture.advanceBoard(); await until(() => !board.dataset.dragging, 'Revision change did not cancel the gesture');
  must(fixture.boardState().requests.length === 1, 'Revision cancellation submitted against a new scope');
  await start(board); fixture.setBoardFlags({ frozen: true }); await until(() => !board.dataset.dragging, 'Frozen plan did not cancel');
  await until(() => board.querySelector('[data-slice-drag="S1"]').disabled, 'Frozen handle remained enabled');
  must(fixture.boardState().requests.length === 1, 'Frozen gesture sent a request');
  fixture.setBoardFlags({ frozen: false });
  await start(board); key('Escape'); await until(() => !board.dataset.dragging, 'First pickup after frozen state did not cleanly cancel');
  const apiRequests = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
  must(apiRequests === 0 && fixture.boardRequests.every(request => request.method === 'POST' && request.path === '/api/chats/1/plan'), 'Status gestures reached another service');
  return { width: document.documentElement.clientWidth, cards: 100, syntheticKeyboard: true, statusRequests: 1, blockedCancellation: true, scopeCancellation: true, apiRequests };
}
async function proposeBlocked(board, panel) {
  await start(board); await lane(board, 'ArrowRight', 'active'); await lane(board, 'ArrowRight', 'in_review'); await lane(board, 'ArrowRight', 'blocked'); await drop(board);
  return until(() => panel.querySelector('.synth-blocked-drop'), 'Blocking proposal missing');
}
const submit = form => form.querySelector('button:not([type="button"])');
export async function exerciseBlockerCommands() {
  const { board, fixture, panel } = await setup(), form = await proposeBlocked(board, panel), input = field(form);
  type(input, 'Synthetic retained reason'); await delay(30);
  fixture.setBoardFlags({ frozen: true }); await until(() => input.readOnly, 'Frozen proposal remained editable');
  form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
  must(fixture.boardState().requests.length === 0, 'Frozen proposal dispatched');
  fixture.setBoardFlags({ frozen: false }); await until(() => !input.readOnly, 'Unfrozen proposal did not recover');
  const planTab = [...panel.querySelectorAll('[role="tab"]')].find(node => node.textContent.trim() === 'Plan'); planTab.click(); await delay(80);
  must(document.contains(input) && input.value === 'Synthetic retained reason' && input.readOnly, 'Hidden proposal lost or exposed its input');
  [...panel.querySelectorAll('[role="tab"]')].find(node => node.textContent.trim() === 'Board').click();
  await until(() => !input.readOnly, 'Revealed proposal stayed locked');
  const preferences = await import('/src/ui-preferences.ts'); preferences.setSynth(false); await delay(80);
  must(document.contains(input) && input.value === 'Synthetic retained reason', 'Skin switch replaced the reason owner');
  preferences.setSynth(true); await until(() => !input.readOnly, 'Synth return did not restore the reason');
  fixture.failBoardStatus('refuse'); submit(form).click();
  await until(() => form.textContent.includes('not confirmed') && !input.readOnly, 'Refusal did not retain its editable reason');
  must(fixture.boardState().snapshot.bundle.slices[0].status === 'ready' && field(form) === input && input.value === 'Synthetic retained reason', 'Refusal optimistically moved or replaced the draft');
  submit(form).click();
  await until(() => fixture.boardState().snapshot.bundle.slices[0].status === 'blocked' && !document.contains(form), 'Explicit retry did not settle the captured proposal');
  await until(() => document.activeElement?.dataset.sliceDrag === 'S1', 'Accepted blocker lost logical focus after returning to its form');
  const requests = fixture.boardState().requests;
  must(requests.length === 2 && requests.every(request => request.expect_revision === 4 && request.key === 'S1' && request.status === 'blocked' && request.reason === 'Synthetic retained reason'), 'Retry borrowed a new revision, target or reason');
  must(fixture.boardState().snapshot.bundle.slices[0].blocked_reason === 'Synthetic retained reason', 'Recorded reason differs');
  const apiRequests = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
  must(apiRequests === 0 && document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, 'Blocker overflow or real API request');
  return { width: document.documentElement.clientWidth, retainedInput: true, frozenAndHiddenGuard: true, explicitSameRevisionRetry: true, syntheticRequests: requests.length, apiRequests };
}
export async function exerciseLostBlockerResponse() {
  const { board, fixture, panel } = await setup(), form = await proposeBlocked(board, panel), input = field(form);
  type(input, 'Synthetic lost-response reason'); await delay(20); fixture.failBoardStatus('lost'); submit(form).click();
  await until(() => fixture.boardState().snapshot.revision === 5 && form.textContent.includes('plan changed') && submit(form).disabled && !input.readOnly, 'Lost response did not retain the stale proposal');
  must(field(form) === input && input.value === 'Synthetic lost-response reason' && fixture.boardState().snapshot.bundle.slices[0].status === 'blocked', 'Unknown response hid the authoritative result or draft');
  form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })); await delay(50);
  must(fixture.boardState().requests.length === 1, 'Unknown outcome automatically retried against a new revision');
  form.querySelector('button[type="button"]').click(); await until(() => !document.contains(form), 'Unknown proposal did not close');
  must(fixture.boardState().requests.length === 1 && fixture.boardState().snapshot.bundle.slices[0].status === 'blocked', 'Cancel undid or re-sent the prior attempt');
  const apiRequests = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
  must(apiRequests === 0, 'Lost-response fixture reached an API');
  return { width: document.documentElement.clientWidth, unknownOutcomeRetained: true, noAutomaticRetry: true, cancelDidNotUndo: true, syntheticRequests: 1, apiRequests };
}
export async function exerciseBoardDisposal() {
  const { board, fixture, panel } = await setup(); await start(board);
  const tab = [...panel.querySelectorAll('[role="tab"]')].find(node => node.textContent.trim() === 'Plan'); tab.focus(); tab.click();
  await until(() => !board.dataset.statusDrag && !document.querySelector('.synth-plan-drag-overlay') && !board.querySelector('[data-source]'), 'Hidden Board retained its input engine or portal');
  await delay(200); let frames = 0; const original = window.requestAnimationFrame;
  window.requestAnimationFrame = callback => { frames++; return original.call(window, callback); };
  try { await delay(500); } finally { window.requestAnimationFrame = original; }
  must(frames === 0 && fixture.boardState().requests.length === 0, `Hidden Board scheduled ${frames} frames or sent a status request`);
  return { width: document.documentElement.clientWidth, hiddenEngineRemoved: true, sampledFrames: frames, syntheticRequests: 0, note: 'Scoped 500ms fixture observation, not whole-app CPU or native acceptance.' };
}
let pointerProbe;
export async function prepareProductionPointer() {
  const { board, fixture } = await setup(), card = board.querySelector('[data-slice-key="S1"]'), frames = [];
  const handle = card.querySelector('[data-slice-drag]'); handle.scrollIntoView({ block: 'center', inline: 'center' });
  await until(() => !handle.disabled, 'Pointer handle unavailable');
  let frame = null, last = null, retained = false;
  const tick = time => { if (last !== null) frames.push(time - last); last = time; frame = requestAnimationFrame(tick); };
  const observer = new MutationObserver(() => {
    if (board.dataset.dragging && frame === null) {
      retained = card.isConnected && board.querySelector('[data-slice-key="S1"]') === card && board.querySelectorAll('[data-slice-key]').length === 100 && fixture.boardState().requests.length === 0;
      frame = requestAnimationFrame(tick);
    } else if (!board.dataset.dragging && frame !== null) { cancelAnimationFrame(frame); frame = null; }
  });
  observer.observe(board, { attributes: true, attributeFilter: ['data-dragging'] });
  pointerProbe = { board, fixture, frames, retained: () => retained, close: () => { observer.disconnect(); if (frame !== null) cancelAnimationFrame(frame); } };
  return { source: 'Move S1', target: 'Collapse Active lane', cards: 100 };
}
export async function readProductionPointer() {
  const probe = must(pointerProbe, 'Prepare the production pointer probe'); pointerProbe = null;
  await until(() => !probe.board.dataset.dragging && probe.fixture.boardState().requests.length === 1, 'Pointer status request did not settle'); probe.close();
  const request = probe.fixture.boardState().requests[0];
  must(probe.retained() && request.key === 'S1' && request.status === 'active' && request.expect_revision === 4, 'Pointer input lost its source or exact status contract');
  must(probe.fixture.boardState().snapshot.bundle.slices[0].status === 'active', 'Pointer result was not reflected');
  const sorted = probe.frames.slice().sort((a, b) => a - b), apiRequests = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
  must(apiRequests === 0, 'Pointer probe reached a real API');
  return { cards: 100, retainedSourceDuringGesture: true, syntheticStatusRequests: 1, pointerFrames: sorted.length, medianMs: sorted[Math.floor(sorted.length / 2)] ?? null, p95Ms: sorted[Math.floor(sorted.length * .95)] ?? null,
    note: 'Bounded Chromium tool-driven pointer interval; not physical/native input, whole-app INP or 60fps acceptance.', apiRequests };
}
