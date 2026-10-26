const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const assert = (ok, message) => { if (!ok) throw Error(message); };
async function until(test, message) {
  const end = performance.now() + 6000;
  while (!test()) { if (performance.now() > end) throw Error(message); await pause(30); }
}
export async function exerciseComposerEqualizer() {
  assert(location.pathname === '/synth-shell-preview.html', 'Use the isolated shell fixture');
  await until(() => document.querySelector('.synth-activity-eq'), 'Equalizer did not load');
  const entry = [...document.scripts].find(script => new URL(script.src, location.href).pathname === '/src/synth/shell-preview.tsx');
  const fixture = await import(entry.src), eq = document.querySelector('.synth-activity-eq');
  const draft = document.querySelector('textarea[aria-label="Message"]');
  const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
  set.call(draft, 'Keep this equalizer draft.'); draft.dispatchEvent(new Event('input', { bubbles: true }));
  const pattern = () => [...eq.querySelectorAll('rect')].map(rect => `${rect.dataset.lit}/${rect.dataset.peak}`).join();
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  assert(eq.querySelectorAll('rect').length === 72 && eq.getAttribute('role') === 'img', 'Equalizer is not a bounded decorative display');
  assert(eq.title.includes('not audio or measured progress') && !eq.querySelector('[role="meter"], [role="progressbar"], [aria-valuenow]'), 'Equalizer implies measured levels');
  for (const state of ['idle', 'running', 'stopping', 'awaiting_approval', 'failed', 'interrupted', 'stopped', 'team_blocked', 'team_interrupted']) {
    fixture.setScopeState(state, true);
    const active = state === 'running' || state === 'stopping';
    await until(() => eq.dataset.active === String(active), `Equalizer disagrees with ${state}`);
    eq.scrollIntoView({ block: 'center' }); await pause(120);
    const before = pattern(); await pause(350);
    assert(active && !reduced ? pattern() !== before : pattern() === before, `Equalizer motion disagrees with ${state}/reduced=${reduced}`);
    assert(eq.getAttribute('aria-label') === (active ? 'Chat working' : 'Chat not running'), 'Accessible activity state is wrong');
    const send = eq.nextElementSibling, box = eq.getBoundingClientRect(), button = send.getBoundingClientRect();
    assert(send.matches('.chat-send') && box.right <= button.left && button.left - box.right <= 8, 'Equalizer is not immediately left of Send');
    assert(Math.abs((box.top + box.bottom - button.top - button.bottom) / 2) < 2, 'Equalizer and Send are misaligned');
    assert(box.width <= 48 && box.height <= 32 && getComputedStyle(eq).pointerEvents !== 'none', 'Equalizer lost its compact readable hint');
    const composer = eq.closest('.chat-composer').getBoundingClientRect();
    for (const control of eq.closest('.chat-composer-actions').querySelectorAll('button')) {
      const r = control.getBoundingClientRect();
      assert(r.left >= composer.left && r.right <= composer.right + 1, 'Activity actions overflow the composer');
      if (matchMedia('(pointer: coarse)').matches) assert(r.width >= 44 && r.height >= 44, 'Activity shrank a touch control');
    }
    assert(document.documentElement.scrollWidth <= innerWidth + 1, 'Activity causes horizontal page overflow');
  }
  fixture.setScopeState('running'); await until(() => eq.dataset.active === 'true', 'Working fixture did not settle');
  eq.scrollIntoView({ block: 'center' }); await pause(150);
  const owner = eq.closest('.chat-composer-area'); owner.hidden = true; await pause(150);
  const hidden = pattern(); await pause(350); assert(pattern() === hidden, 'Hidden composer keeps animating');
  owner.hidden = false; eq.scrollIntoView({ block: 'center' }); await pause(150);
  const revealed = pattern(); await pause(350);
  assert(reduced ? pattern() === revealed : pattern() !== revealed, 'Visible/reduced equalizer does not resume correctly');
  fixture.setScopeState('idle', true); await until(() => eq.dataset.active === 'false', 'Idle equalizer did not settle');
  assert([...eq.querySelectorAll('rect')].every(rect => rect.dataset.lit === 'false' && rect.dataset.peak === 'false'), 'Idle equalizer retained live-looking bars');
  assert(document.querySelector('.synth-activity-eq') === eq && document.querySelector('textarea[aria-label="Message"]') === draft && draft.value === 'Keep this equalizer draft.', 'Activity replaced its controls or draft');
  assert(fixture.fixtureCommands.length === 0, 'Activity dispatched a command');
  assert(!performance.getEntriesByType('resource').some(entry => new URL(entry.name).pathname.startsWith('/api/')), 'A real API request escaped the fixture');
  return { states: 9, reduced, hiddenFrozen: true, retainedDraft: true, segments: 72, commands: 0 };
}
