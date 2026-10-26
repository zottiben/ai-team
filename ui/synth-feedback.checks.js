/** Geometry checks for the operator's 20 October review; run on the isolated real-App fixture. */
export function inspectFeedbackLayout() {
  const errors = [], visible = selector => [...document.querySelectorAll(selector)].filter(el => el.checkVisibility());
  const assert = (ok, message) => { if (!ok) errors.push(message); };
  for (const shell of visible('.chat-transcript-shell')) {
    const owner = shell.closest('.chat-conversation').getBoundingClientRect(), r = shell.getBoundingClientRect(), transcript = shell.querySelector('.chat-transcript').getBoundingClientRect();
    assert(r.right <= owner.right + 1 && r.left >= owner.left - 1, 'Session updates escape their column');
    assert(transcript.bottom <= r.bottom + 1, 'Session updates overlap the composer or outcome');
    assert(transcript.height >= 250, 'Session updates are squeezed too short');
    for (const sibling of shell.closest('.chat-conversation').querySelectorAll('.chat-composer, .chat-outcome')) {
      if (sibling.checkVisibility()) assert(sibling.getBoundingClientRect().top >= r.bottom - 1, 'Updates paint over the composer or outcome');
    }
  }
  for (const scroll of visible('.synth-sequence-scroll')) {
    const grid = scroll.firstElementChild;
    assert(grid.getBoundingClientRect().width >= scroll.clientWidth - 2, 'Session pattern leaves unused width');
    assert(scroll.scrollWidth - scroll.clientWidth - scroll.scrollLeft < 4, 'Session pattern does not follow latest step');
  }
  for (const board of visible('.plan-kanban')) {
    const lanes = [...board.querySelectorAll('.plan-lane')];
    assert(lanes.at(-1).getBoundingClientRect().right >= board.getBoundingClientRect().right - 18 || board.scrollWidth > board.clientWidth, 'Board leaves unused width');
  }
  for (const form of visible('.diff__compose')) {
    const field = form.querySelector('textarea');
    assert(field.clientWidth >= form.clientWidth - 40 && field.clientHeight >= 100, 'Review comment input is cramped');
    assert(!!form.querySelector('[aria-label="Close comment"]'), 'Review comment has no close control');
  }
  for (const modal of visible('.chat-checkout-popover')) {
    const r = modal.getBoundingClientRect();
    assert(modal.matches('dialog:modal'), 'Checkout is not a modal dialog');
    assert(Math.abs((r.left + r.right) / 2 - innerWidth / 2) < 2 && Math.abs((r.top + r.bottom) / 2 - innerHeight / 2) < 2, 'Checkout is not centered');
  }
  for (const lane of visible('.synth-arrangement__lane')) {
    const owner = lane.closest('.synth-arrangement__scroll');
    assert(!!owner && getComputedStyle(owner).overflowX === 'auto', 'Arrangement has no bounded horizontal scroll owner');
  }
  for (const composer of visible('.chat-composer')) {
    // Coarse input retains 44px targets rather than shrinking to a desktop budget.
    const coarse = matchMedia('(pointer: coarse)').matches;
    assert(composer.getBoundingClientRect().height <= (coarse ? 250 : 230), 'Composer consumes transcript space');
    if (coarse) for (const target of composer.querySelectorAll('button, summary, [role="slider"]')) {
      if (target.checkVisibility()) { const box = target.getBoundingClientRect(); assert(box.width >= 44 && box.height >= 44, `Compact composer shrank ${target.getAttribute('aria-label') ?? target.textContent}: ${box.width}×${box.height}`); }
    }
    assert(!visible('.synth-thinking-control .synth-knob-labels').length, 'Thinking dial still has visual detent labels');
  }
  if (errors.length) throw Error(errors.join('; '));
  return { checked: true };
}

const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const insist = (condition, message) => { if (!condition) throw Error(message); };
async function settled(test, label) {
  const end = performance.now() + 6000;
  while (!test()) { if (performance.now() > end) throw Error(label); await pause(30); }
}
export async function exerciseFeedbackScope() {
  const entry = [...document.scripts].find(script => new URL(script.src, location.href).pathname === '/src/synth/shell-preview.tsx');
  const fixture = await import(entry.src);
  const scope = document.querySelector('.synth-token-scope');
  scope.scrollIntoView({ block: 'center' }); await pause(200);
  const graph = scope.querySelector('.synth-graph'), phase = () => graph.style.backgroundPositionX;
  graph.scrollIntoView({ block: 'center' }); await pause(200);
  const idle = phase(); await pause(250); insist(phase() === idle, 'An idle chat kept its scope running');
  fixture.setScopeState('running'); await settled(() => graph.dataset.active === 'true', 'Scope did not follow active chat'); await pause(200);
  const before = phase(); await pause(250);
  insist(matchMedia('(prefers-reduced-motion: reduce)').matches ? phase() === before : phase() !== before, 'Trace did not respect visible/reduced motion ownership');
  fixture.advanceOverviewTelemetry();
  await settled(() => scope.textContent.replaceAll(',', '').includes('1300 prompt tokens'), 'Live readings did not advance');
  insist(scope.textContent.includes('350 input tokens') && scope.textContent.includes('75 output tokens'), 'Reported levels were lost');
  const rack = document.querySelector('.synth-overview-rack');
  insist(rack.textContent.includes('Uncached input: 200') && rack.textContent.includes('Output: 45'), 'Display motion changed accounting');
  const pane = scope.closest('.chat-context-pane');pane.hidden = true;await pause(200);
  const stopped = phase();await pause(250);insist(phase() === stopped, 'Hidden trace continued moving');pane.hidden = false;
  const scroll = document.querySelector('.synth-sequence-scroll');scroll.scrollIntoView({block:'center'});await pause(100);
  if (scroll.scrollWidth > scroll.clientWidth) {
    scroll.scrollLeft = 0;scroll.dispatchEvent(new Event('scroll', {bubbles:true}));await pause(50);
    const first = scroll.querySelector('[data-pattern-id]').dataset.patternId;
    fixture.appendActivity();await pause(300);
    insist(scroll.scrollLeft === 0 && scroll.querySelector('[data-pattern-id]').dataset.patternId === first, 'New activity stole the held pattern range');
    [...document.querySelectorAll('button')].find(button => button.textContent === 'Follow latest').click();
    await settled(() => scroll.scrollWidth-scroll.clientWidth-scroll.scrollLeft < 4, 'Follow latest did not restore the live tail');
  }
  const prior = scroll.querySelector('[data-pattern-id]:last-child').dataset.patternId;
  fixture.appendActivity();await settled(() => scroll.querySelector('[data-pattern-id]:last-child').dataset.patternId !== prior, 'New activity did not reach the pattern');
  insist(scroll.scrollWidth-scroll.clientWidth-scroll.scrollLeft < 4, 'New activity lost tail following');
  insist(fixture.fixtureCommands.length === 0, 'Presentation dispatched a command');
  return { readings:'350/75/1300', canonical:'200/45', heldRange:true, commands:0 };
}
export async function exerciseFeedbackComment() {
  const form = document.querySelector('.diff__compose:not([hidden])'), field = form.querySelector('textarea');
  const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
  set.call(field, 'Keep this review draft.');field.dispatchEvent(new Event('input', {bubbles:true}));
  form.querySelector('[aria-label="Close comment"]').click();await pause(30);
  insist(form.hidden && field.isConnected && field.value === 'Keep this review draft.', 'Closing discarded the review draft');
  [...document.querySelectorAll('button')].find(button => button.textContent === 'Resume comment').click();await pause(30);
  insist(!form.hidden && document.activeElement === field && field.value === 'Keep this review draft.', 'Resume replaced the comment owner');
  return { retainedComment:true };
}
