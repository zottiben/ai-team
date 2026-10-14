const pause = () => new Promise(resolve => setTimeout(resolve, 40));
const assert = (condition, message) => { if (!condition) throw new Error(message); };
async function until(test) {
  for (let i = 0; i < 125; i++) { const value = test(); if (value) return value; await pause(); }
  throw new Error('Composer fixture did not settle');
}
export async function exerciseComposer() {
  assert(location.pathname === '/synth-shell-preview.html', 'Use the isolated shell fixture');
  const knob = await until(() => document.querySelector('[role="slider"][aria-label="Thinking effort"][aria-disabled="false"]'));
  const draft = document.querySelector('textarea[aria-label="Message"]');
  const value = draft.value;
  assert(draft.getBoundingClientRect().height >= 72, 'Composer LCD is shorter than the minimum');
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
  return {width: document.documentElement.clientWidth, textareaHeight: draft.getBoundingClientRect().height, exactCapabilities: true, retainedDraft: true, apiRequests: 0};
}
