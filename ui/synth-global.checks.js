// Real computed styles and geometry, on the network-disabled real-App fixture only.
export function inspectGlobalSurface(name) {
  const root = name === 'Notifications' ? document.querySelector('.notification-center__panel') : document.querySelector('.chat-main > .main');
  if (!root) throw Error(`Missing ${name} surface`);
  const visible = element => element.getClientRects().length && !element.closest('[hidden]') && getComputedStyle(element).visibility !== 'hidden';
  if (!getComputedStyle(root).fontFamily.includes('Figtree')) throw Error(`${name} did not adopt Synth typography`);
  const problems = [];
  if (document.documentElement.scrollWidth > innerWidth + 1 || root.scrollWidth > root.clientWidth + 2) problems.push('page overflow');
  const cards = [...root.querySelectorAll('.card, .settings__group, .main__header, .plan-library-heading, .today__masthead, .notification-center__item')].filter(visible);
  if (!cards.length) problems.push('no styled surface');
  for (const card of cards) {
    const style = getComputedStyle(card), box = card.getBoundingClientRect();
    if (parseFloat(style.borderRadius) < 12 || style.boxShadow === 'none') problems.push(`legacy card: ${card.className}`);
    if (box.right > root.getBoundingClientRect().right + 2) problems.push(`card overflows: ${card.className}`);
  }
  const coarse = matchMedia('(pointer:coarse)').matches;
  for (const button of [...root.querySelectorAll('.button, .nav-item')].filter(visible)) {
    const style = getComputedStyle(button), box = button.getBoundingClientRect();
    if (!style.fontFamily.includes('Figtree') || parseFloat(style.borderRadius) < 12) problems.push(`legacy control: ${button.textContent}`);
    if (box.height < (coarse ? 44 : 32) - 1) problems.push(`small control: ${button.textContent} ${box.height}`);
  }
  if ([...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Project tools')) problems.push('Project tools remains');
  if (problems.length) throw Error(`${name}: ${[...new Set(problems)].join('; ')}`);
  return { name, cards: cards.length, width: root.clientWidth, scrollWidth: root.scrollWidth, coarse };
}

export async function exerciseExistingModel() {
  const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
  const entry = [...document.scripts].find(script => script.type === 'module' && script.src.includes('/src/synth/shell-preview.tsx'));
  if (!entry) throw Error('Missing exact fixture entry');
  const fixture = await import(entry.src);
  fixture.setScopeState('running'); await wait(500);
  const field = document.querySelector('textarea[aria-label="Message"]');
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
  setter.call(field, 'Retain this unsent model-switch draft'); field.dispatchEvent(new Event('input', { bubbles: true }));
  const button = document.querySelector('button[aria-label="Model"]');
  if (!button || button.disabled) throw Error('Existing running model cannot be selected');
  button.click(); await wait(100);
  [...document.querySelectorAll('[role="option"]')].find(option => option.textContent.includes('fixture-small')).click();
  await wait(600);
  if (document.querySelector('textarea[aria-label="Message"]') !== field || field.value !== 'Retain this unsent model-switch draft') throw Error('Model switch replaced the draft');
  if (!button.textContent.includes('fixture-small')) throw Error('Saved model not rendered');
  if (fixture.fixtureCommands.length !== 1 || fixture.fixtureCommands[0].path !== '/api/chats/1/model') throw Error('Selection stopped, sent or otherwise executed work');
  const graphs = [...document.querySelectorAll('.synth-graph[data-flow]')];
  if (!graphs.length || graphs.some(graph => graph.dataset.active !== 'true')) throw Error('Selection changed owning chat liveness');
  fixture.setScopeState('idle'); await wait(500);
  return { retainedDraft: true, modelRequests: fixture.fixtureCommands.length, stoppedOrSent: false };
}
