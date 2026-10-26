import { checkShell } from './synth-shell.checks.js';
const wait = async predicate => {
  const end = performance.now() + 10000;
  while (!predicate()) {
    if (performance.now() > end) throw new Error('Work fixture did not settle');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
};
export async function exerciseWork() {
  if (location.pathname !== '/synth-shell-preview.html' || new URLSearchParams(location.search).get('work') !== 'true') throw new Error('Use the isolated Work fixture');
  const tab = () => [...document.querySelectorAll('[role="tab"]')].find(el => el.textContent.trim() === 'Work');
  await wait(() => tab() && !tab().disabled);
  tab().click();
  await wait(() => document.querySelectorAll('.synth-turn').length === 6);
  await document.fonts.ready;
  const rows = document.querySelector('.synth-turns');
  const filter = [...document.querySelectorAll('[aria-label="Filter turns"] button')].find(el => el.textContent.startsWith('Done'));
  filter.click();
  await wait(() => rows.children.length === 1);
  document.querySelector('.synth-clip[aria-label^="Run #3 ·"]').click();
  await wait(() => document.activeElement?.dataset.runId === '3');
  const target = document.activeElement.getBoundingClientRect(), viewport = rows.getBoundingClientRect();
  if (target.bottom <= viewport.top || target.top >= viewport.bottom) throw new Error('Exact run is focused but not visible');
  if (document.querySelector('.synth-arrangement__ruler')) throw new Error('Unknown endpoints acquired a ruler');
  if (document.querySelector('.synth-clip[data-live]')) throw new Error('Historical running row became live');
  rows.querySelector('[data-run-id="3"] .synth-turn__more').click();
  await wait(() => rows.querySelector('[data-run-id="3"] .synth-turn__prompt-full'));
  const { setSynth } = await import('/src/ui-preferences.ts');
  setSynth(false); await wait(() => !tab());
  setSynth(true); await wait(() => tab());
  if (document.querySelector('.synth-turns') !== rows || !rows.querySelector('[data-run-id="3"] .synth-turn__prompt-full')) throw new Error('Skin toggle lost Work state');
  const shell = await checkShell();
  return { ...shell, rows: rows.children.length, exactFocusVisible: true, retained: true, unknownsPreserved: true };
}
