const pause = () => new Promise(resolve => setTimeout(resolve, 40));
const assert = (condition, message) => { if (!condition) throw new Error(message); };
async function until(test) {
  for (let i = 0; i < 125; i++) { const value = test(); if (value) return value; await pause(); }
  throw new Error('Composer fixture did not settle');
}
export function inspectComposerControlRow() {
  const composer = document.querySelector('.chat-composer');
  const selectors = ['.chat-attach-image', '.synth-execution-policy summary', 'button[aria-label="Model"]', '[role="slider"][aria-label="Thinking effort"]', 'button[aria-label="Refresh thinking levels"]', '.chat-send'];
  const controls = selectors.map(selector => composer.querySelector(selector));
  assert(controls.every(Boolean), `Composer controls unavailable: ${selectors.filter((_, index) => !controls[index]).join(', ')}`);
  const boxes = controls.map(control => control.getBoundingClientRect());
  const centers = boxes.map(box => (box.top + box.bottom) / 2);
  const spread = Math.max(...centers) - Math.min(...centers);
  const box = composer.getBoundingClientRect();
  if (box.width >= 800) assert(spread <= 2, `Wide composer unnecessarily stacks controls (${spread}px row spread)`);
  assert(boxes.every(control => control.left >= box.left && control.right <= box.right + 1 && control.bottom <= box.bottom + 1), 'Composer control escapes its plate');
  const coarse = matchMedia('(pointer: coarse)').matches;
  if (coarse) for (const control of boxes) assert(control.width >= 44 && control.height >= 44, 'Inline controls lost touch targets');
  const notice = composer.querySelector('small.chat-model-notice');
  assert(notice?.checkVisibility() && notice.title.includes('Running and already queued turns keep their model'), 'Future-turn model boundary was hidden');
  assert(box.height <= (coarse ? 250 : 230), 'Composer exceeds its compact height budget');
  return { width: box.width, height: box.height, rowSpread: spread, coarse };
}

export async function exerciseComposer() {
  assert(location.pathname === '/synth-shell-preview.html', 'Use the isolated shell fixture');
  const knob = await until(() => document.querySelector('[role="slider"][aria-label="Thinking effort"][aria-disabled="false"]'));
  await until(() => document.querySelector('.chat-composer button[aria-label="Model"]') && document.querySelector('.chat-composer small.chat-model-notice'));
  await until(() => document.querySelector('.synth-activity-eq'));
  await document.fonts.ready;
  const draft = document.querySelector('textarea[aria-label="Message"]');
  const value = draft.value;
  const layout = inspectComposerControlRow();
  const height = draft.getBoundingClientRect().height;
  assert(height >= 48 && height <= 80, 'Composer LCD lost the compact operator-requested size');
  knob.focus();
  knob.dispatchEvent(new KeyboardEvent('keydown', {key: 'Home', bubbles: true}));
  assert(knob.getAttribute('aria-valuetext') === 'Off', 'Home missed the actual Off capability');
  knob.dispatchEvent(new KeyboardEvent('keydown', {key: 'End', bubbles: true}));
  assert(knob.getAttribute('aria-valuetext') === 'High', 'End invented a level not offered by the fixture model');
  await new Promise(resolve => setTimeout(resolve, 300)); // Returning to the saved value submits nothing.
  const mode = document.querySelector('button[aria-label="Execution mode"]');
  mode.click();
  const choices = await until(() => document.querySelector('[role="listbox"][aria-label="Execution mode"]'));
  await until(() => choices.querySelector('[aria-selected="true"]') === document.activeElement);
  choices.dispatchEvent(new KeyboardEvent('keydown', {key: 'ArrowDown', bubbles: true}));
  assert(document.activeElement?.textContent === 'Team · Pi', 'Mode choices lost keyboard navigation');
  choices.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));
  await until(() => document.activeElement === mode);
  assert(document.querySelector('textarea[aria-label="Message"]') === draft && draft.value === value, 'Composer controls replaced the draft');
  assert(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, 'Composer caused page overflow');
  assert(!performance.getEntriesByType('resource').some(entry => new URL(entry.name).pathname.startsWith('/api/')), 'A real API request escaped the fixture');
  return {width: document.documentElement.clientWidth, textareaHeight: draft.getBoundingClientRect().height, layout, exactCapabilities: true, retainedDraft: true, apiRequests: 0};
}
