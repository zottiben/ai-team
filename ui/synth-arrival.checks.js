import { shellFixture } from './synth-fixture.checks.js';
const pause = () => new Promise(resolve => setTimeout(resolve, 40));
const must = (value, message) => { if (!value) throw Error(message); };
async function until(test, message) { for (let index = 0; index < 125; index++) { const value = test(); if (value) return value; await pause(); } throw Error(message); }
const shown = element => element?.getClientRects().length > 0 && !element.closest('[hidden]');
export async function exerciseReceiptMotion() {
  const params = new URLSearchParams(location.search);
  must(location.pathname === '/synth-shell-preview.html' && params.get('activity') === '5000' && params.get('activity-motion') === 'true', 'Use the isolated 5000-step activity-motion fixture');
  must(document.visibilityState === 'visible' && !matchMedia('(prefers-reduced-motion: reduce)').matches, 'Visible motion-enabled viewport required');
  const overview = await until(() => [...document.querySelectorAll('[role=tab]')].find(node => node.textContent === 'Overview'), 'Overview unavailable');
  const fixture = await shellFixture(); overview.click();
  const sequence = await until(() => document.querySelector('.synth-sequence-scroll'), 'Sequence unavailable');
  await until(() => document.querySelector('.synth-pattern-heading')?.textContent.includes('of 5000 mapped'), 'Complete 5000-step history did not load');
  sequence.scrollLeft = sequence.scrollWidth;
  [...sequence.querySelectorAll('button')].at(-1).scrollIntoView({ block: 'nearest', inline: 'nearest' });
  document.querySelector('.chat-follow')?.click(); await pause(); await pause();
  const original = Element.prototype.animate, recorded = [];
  Element.prototype.animate = function(frames, options) {
    const animation = original.call(this, frames, options);
    const record = { animation, element: this, frames, options, sample: null, finished: animation.finished.then(() => true, () => false) };
    recorded.push(record);
    requestAnimationFrame(() => { record.sample = [getComputedStyle(this).translate, getComputedStyle(this).scale]; });
    return animation;
  };
  const owned = () => recorded.filter(item => item.animation.id.startsWith('synth-'));
  try {
    const focus = document.activeElement, id = fixture.appendShellPhase('start', '');
    const pending = await until(() => sequence.querySelector('button[aria-current="step"]'), 'Exact pending step was not marked');
    must(pending.title.includes(`event ${id}`) && pending.title.includes('not proof of liveness'), 'Pending ring acquired another identity or implied liveness');
    await until(() => owned().some(item => item.animation.id === 'synth-receipt-arrival') && owned().some(item => item.animation.id === 'synth-pattern-arrival'), 'Visible receipt/pattern motion did not start');
    const arrivals = [...owned()];
    must(arrivals.length <= 48 && arrivals.every(item => item.options.iterations === 1 && [160, 180].includes(item.options.duration)), 'Arrival effects exceeded their finite bounds');
    let timer;
    const completed = await Promise.race([Promise.all(arrivals.map(item => item.finished)), new Promise((_, reject) => { timer = setTimeout(() => reject(Error('Finite receipt motion did not settle')), 3000); })]).finally(() => clearTimeout(timer));
    must(completed.every(Boolean), 'Arrival effects did not finish naturally');
    must(arrivals.some(item => item.sample && (item.sample[0].split(' ').some(value => Math.abs(parseFloat(value) || 0) > .001) || Math.abs((parseFloat(item.sample[1]) || 1) - 1) > .001)), 'Native animation changed no visual transform');
    must(document.activeElement === focus, 'Receipt arrival stole focus');
    const finishedCount = owned().length;
    fixture.appendShellPhase('partial', 'A synthetic sample, not a process');
    await until(() => document.querySelector(`[data-row-id="${id}"]`)?.textContent.includes('sample'), 'Partial receipt was not observed');
    await pause(); must(owned().length === finishedCount, 'A sampled update replayed row entry');
    fixture.appendShellPhase('end', 'Synthetic completed output');
    await until(() => !sequence.querySelector('[aria-current="step"]'), 'Completed receipt retained the pending pulse');
    must(owned().length === finishedCount, 'A completion replayed row entry');

    pending.click();
    const cue = await until(() => document.querySelector(`[data-row-id="${id}"] .synth-tracker-focus`), 'Exact jump cue unavailable');
    must(getComputedStyle(cue).animationDuration === '0.6s', 'Jump cue lost its 600ms duration');
    const opacity = Number(getComputedStyle(cue).opacity); await pause(); await pause();
    must(Number(getComputedStyle(cue).opacity) < opacity, 'Jump cue did not visibly fade');
    pending.click();
    await until(() => { const next = document.querySelector(`[data-row-id="${id}"] .synth-tracker-focus`); return next && next !== cue; }, 'Repeated jump reused an older cue owner');

    const first = sequence.querySelector('button'); first.scrollIntoView({ block: 'nearest', inline: 'nearest' }); first.focus({ preventScroll: true }); await pause();
    fixture.appendActivity();
    await until(() => document.querySelector('.synth-pattern-heading')?.textContent.includes('held for keyboard focus'), 'Tail advancement discarded its focus owner');
    must(first.isConnected && document.activeElement === first, 'Tail window removed the focused control');
    const editor = document.querySelector('[data-chat-tool="editor"]'); editor.click();
    await until(() => !shown(document.querySelector('.chat-transcript')), 'Tool did not hide its retained transcript');
    const beforeHidden = owned().length;
    const total = Number(document.querySelector('.synth-pattern-heading').textContent.match(/of (\d+) mapped/)[1]);
    fixture.appendActivity();
    await until(() => document.querySelector('.synth-pattern-heading').textContent.includes(`of ${total + 1} mapped`), 'Hidden controller did not receive the fixture update');
    fixture.setActivityRunning(false); await pause(); editor.click();
    await until(() => shown(document.querySelector('.chat-transcript')), 'Transcript did not return'); await pause();
    must(owned().length === beforeHidden, 'Hidden arrivals replayed on reveal');
    must(!sequence.querySelector('[aria-current="step"]'), 'Inactive turn kept an observed-current ring');
    const apiRequests = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
    must(apiRequests === 0, 'Receipt checks reached a real backend');
    return { finiteArrivals: arrivals.length, exactPending: true, repeatedJumpCue: true, focusedTailRetained: true, noUpdateReplay: true, hiddenNoReplay: true, apiRequests };
  } finally { Element.prototype.animate = original; }
}

export async function exerciseReducedReceipts() {
  must(location.pathname === '/synth-shell-preview.html' && new URLSearchParams(location.search).get('activity-motion') === 'true', 'Use the isolated activity-motion fixture');
  must(document.visibilityState === 'visible' && matchMedia('(prefers-reduced-motion: reduce)').matches, 'Use real browser reduced-motion emulation, not a patched matchMedia');
  const tab = await until(() => [...document.querySelectorAll('[role=tab]')].find(node => node.textContent === 'Overview'), 'Overview unavailable'); tab.click();
  await until(() => document.querySelector('.synth-pattern-heading')?.textContent.includes('of 5000 mapped'), 'Complete history unavailable');
  const fixture = await shellFixture(), original = Element.prototype.animate, raf = window.requestAnimationFrame;
  let effects = 0, frames = 0;
  Element.prototype.animate = function(...args) { effects++; return original.apply(this, args); };
  try {
    const id = fixture.appendShellPhase('start', '');
    await until(() => document.querySelector('.synth-step[aria-current="step"]')?.title.includes(`event ${id}`), 'Reduced motion suppressed the recorded pending state');
    await pause(); await pause();
    must(effects === 0, 'Reduced motion started finite decoration');
    must(document.getAnimations().filter(animation => animation.playState === 'running').length === 0, 'Reduced motion left a CSS animation running');
    window.requestAnimationFrame = callback => { frames++; return raf.call(window, callback); };
    await new Promise(resolve => setTimeout(resolve, 500));
    const idleFrames = frames; window.requestAnimationFrame = raf;
    must(idleFrames === 0, 'Reduced-motion idle fixture kept requesting animation frames');
    fixture.appendShellPhase('end', 'Completed without decoration');
    await until(() => !document.querySelector('.synth-step[aria-current="step"]'), 'Reduced motion hid the recorded completion');
    const apiRequests = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
    must(apiRequests === 0, 'Reduced-motion fixture reached a real backend');
    return { effects, idleFrames, recordedStatePreserved: true, apiRequests, note: 'Scoped 500ms browser sample, not whole-app CPU certification.' };
  } finally { Element.prototype.animate = original; window.requestAnimationFrame = raf; }
}
