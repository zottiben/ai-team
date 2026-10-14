import { checkShell } from './synth-shell.checks.js';

const waitFor = async test => {
  const until = performance.now() + 10000;
  while (!test()) {
    if (performance.now() > until) throw new Error('Shell fixture did not become ready');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
};
const tab = name => [...document.querySelectorAll('[role="tab"]')].find(el => el.textContent.trim() === name);
const button = name => [...document.querySelectorAll('button')].find(el => el.textContent.trim() === name && el.getClientRects().length);

/** DOM-driven smoke on the real React tree; not a claim of native pointer/PTY testing. */
export async function exerciseWorkspace() {
  if (location.pathname !== '/synth-shell-preview.html') throw new Error('Use the isolated shell fixture');
  await waitFor(() => tab('Work') && !tab('Work').disabled && document.querySelector('textarea[aria-label="Message"]'));
  const draft = document.querySelector('textarea[aria-label="Message"]');
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(draft, 'Retained fixture draft');
  draft.dispatchEvent(new Event('input', { bubbles: true }));
  tab('Work').click();
  await waitFor(() => document.querySelector('[aria-label="Panel view"]'));
  const work = await checkShell();
  tab('Board').click();
  await waitFor(() => document.querySelector('.chat-body')?.dataset.synthBoard === 'true');
  const board = await checkShell();
  button('Editor').click();
  await waitFor(() => button('Editor')?.getAttribute('aria-current') === 'true');
  button('Terminal').click();
  await waitFor(() => button('Terminal')?.getAttribute('aria-current') === 'true');
  if ([...document.querySelectorAll('[role="tab"]')].some(el => el.getAttribute('aria-selected') === 'true')) throw new Error('A tab stayed selected with a tool open');
  if (document.querySelector('[aria-label="Chats"]')?.getAttribute('aria-current') !== 'page') throw new Error('Tool left Chats navigation');
  const { setSynth } = await import('/src/ui-preferences.ts');
  setSynth(false); await waitFor(() => !tab('Work'));
  setSynth(true); await waitFor(() => tab('Work'));
  if (button('Terminal')?.getAttribute('aria-current') !== 'true') throw new Error('Skin toggle closed the tool');
  button('Close tools').click();
  await waitFor(() => tab('Board')?.getAttribute('aria-selected') === 'true');
  if (document.activeElement !== tab('Board')) throw new Error('Closing the tool lost keyboard focus');
  tab('Work').click();
  await waitFor(() => document.querySelector('[aria-label="Panel view"]'));
  if (document.querySelector('textarea[aria-label="Message"]') !== draft || draft.value !== 'Retained fixture draft') throw new Error('Lost or remounted chat draft');
  if (localStorage.getItem('ai-team.theme') !== 'dark') throw new Error('Skin replaced the legacy theme preference');
  const separator = document.querySelector('[role="separator"]');
  if (separator?.getClientRects().length) {
    separator.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
    if (localStorage.getItem('ui.synth.panel-width') !== separator.getAttribute('aria-valuenow')) throw new Error('Keyboard resize was not persisted');
  }
  return { work, board, restored: await checkShell(), draftRetained: true };
}
