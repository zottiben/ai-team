// Bounded DOM/layout probes for the network-disabled real-App fixture only.
const pause = () => new Promise(resolve => setTimeout(resolve, 40));
async function until(test, label) {
  for (let i = 0; i < 125; i++) { const value = test(); if (value) return value; await pause(); }
  throw new Error(label);
}
function assert(value, message) { if (!value) throw new Error(message); }
function isolated() {
  assert(location.pathname === '/synth-shell-preview.html' && Number(new URLSearchParams(location.search).get('activity')) === 5000, 'Use the 5000-activity isolated shell fixture');
  assert(!performance.getEntriesByType('resource').some(entry => new URL(entry.name).pathname.startsWith('/api/')), 'A real API request escaped the fixture');
}
function nonoverlapping(scroll) {
  const rects = [...scroll.querySelectorAll('.chat-transcript-row')].map(row => row.getBoundingClientRect()).sort((a, b) => a.top - b.top);
  for (let i = 1; i < rects.length; i++) assert(rects[i].top >= rects[i - 1].bottom - 1, 'Measured tracker rows overlap');
}
export async function checkTracker() {
  isolated();
  const scroll = await until(() => document.querySelector('.chat-transcript'), 'Missing transcript');
  await until(() => scroll.querySelector('[data-windowed="true"]') && scroll.querySelector('.synth-tracker-tool'), 'Tracker did not window 5k rows');
  await pause();
  assert(scroll.clientHeight > 40, 'Tracker lost its viewport height');
  assert(scroll.querySelectorAll('.chat-transcript-row').length < 80, 'Tracker mounted the complete history');
  assert(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, 'Tracker causes page overflow');
  nonoverlapping(scroll);
  return { mountedRows: scroll.querySelectorAll('.chat-transcript-row').length, viewport: scroll.clientHeight, historyHeight: scroll.scrollHeight, apiRequests: 0 };
}
export async function exerciseGroupedTracker() {
  isolated();
  const thinking = new URLSearchParams(location.search).get('activity-mode') === 'thinking';
  await until(() => document.querySelector(thinking ? '.synth-tracker-thinking' : '[aria-label="Grouped read operations"]'), 'Missing grouped activity');
  const overview = [...document.querySelectorAll('[role="tab"]')].find(el => el.textContent === 'Overview');
  overview.click();
  await until(() => document.querySelector('.synth-sequence-scroll'), 'Missing grouped sequence');
  const sequence = document.querySelector('.synth-sequence-scroll');
  sequence.scrollLeft = sequence.scrollWidth; await pause(); await pause();
  const pad = [...sequence.querySelectorAll('button')].at(-1);
  const target = pad.getAttribute('aria-label').match(/event (\d+)/)[1];
  pad.scrollIntoView({ block: 'nearest', inline: 'nearest' }); await pause(); pad.click();
  const selector = thinking ? '[data-thinking-id]' : '[data-read-id]';
  const attribute = thinking ? 'thinkingId' : 'readId';
  await until(() => document.activeElement?.dataset[attribute] === target, 'Grouped pad did not focus the exact activity');
  const selected = document.activeElement.getBoundingClientRect(), feed = document.querySelector('.chat-transcript').getBoundingClientRect();
  assert(selected.bottom > feed.top && selected.top < feed.bottom, 'Grouped selection is outside the transcript viewport');
  assert(document.querySelectorAll(selector).length < 80, 'One grouped row mounted all 5k children');
  const focused = document.activeElement;
  focused.closest('.synth-tracker-group-scroll').scrollTop = 0;
  await pause(); await pause();
  assert(focused.isConnected && document.activeElement === focused, 'Grouped virtualization discarded the focused item');
  isolated();
  return { pinnedFocus: true, grouped: thinking ? 'thinking' : 'reads', target, mountedChildren: document.querySelectorAll(selector).length, exactFocus: true, apiRequests: 0 };
}
export async function exerciseTracker() {
  await checkTracker();
  const scroll = document.querySelector('.chat-transcript');
  scroll.scrollTop = scroll.scrollHeight / 2;
  await until(() => document.querySelector('.chat-follow'), 'Scrolling away did not hold the tracker');
  await pause();
  const held = scroll.scrollTop;
  const { appendActivity } = await import('/src/synth/shell-preview.tsx');
  appendActivity();
  await until(() => /↓ [1-9]/.test(document.querySelector('.chat-follow')?.textContent ?? ''), 'New activity key did not count arrivals');
  assert(Math.abs(scroll.scrollTop - held) < 2, 'An arrival moved the held viewport');
  const bounds = scroll.getBoundingClientRect();
  const row = [...scroll.querySelectorAll('.chat-transcript-row')].find(el => el.getBoundingClientRect().top < bounds.bottom && el.getBoundingClientRect().bottom > bounds.top && el.querySelector('summary'));
  assert(row, 'No expandable tool row in viewport');
  const summary = row.querySelector('summary'), key = row.dataset.rowId, height = row.getBoundingClientRect().height;
  scroll.scrollTop += summary.getBoundingClientRect().top - bounds.top - 20;
  await pause();
  summary.click();
  await until(() => row.getBoundingClientRect().height > height + 20, 'Expanded output was not measured');
  await pause(); nonoverlapping(scroll);
  const expandedOffset = scroll.scrollTop;
  summary.focus();
  scroll.scrollTop = 0; await pause(); await pause();
  assert(document.activeElement === summary && summary.isConnected, 'Virtualization discarded the focused control');
  summary.blur(); await pause();
  scroll.scrollTop = expandedOffset;
  await until(() => scroll.querySelector(`[data-row-id="${key}"] details[open]`), 'Recycled output lost disclosure state');
  const overview = [...document.querySelectorAll('[role="tab"]')].find(el => el.textContent === 'Overview');
  overview.click();
  await until(() => document.querySelector('[aria-label="Session pattern"]'), 'Overview lost its pattern');
  const all = [...document.querySelectorAll('[aria-label="Pattern range"] button')].find(el => el.textContent === 'All');
  all.click(); await pause();
  const sequence = document.querySelector('.synth-sequence-scroll');
  sequence.scrollLeft = sequence.scrollWidth;
  await until(() => [...document.querySelectorAll('.synth-sequence-column')].some(el => parseFloat(el.style.transform.replace('translateX(', '')) > 200000), 'All pattern did not virtualize/scroll to the final steps');
  const pads = [...document.querySelectorAll('.synth-sequence-column button')];
  const last = pads.at(-1), target = last.getAttribute('aria-label').match(/event (\d+)/)[1];
  last.focus();
  sequence.scrollLeft = 0; await pause(); await pause();
  assert(last.isConnected && document.activeElement === last, 'Horizontal virtualization discarded the focused pad');
  last.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  await pause();
  last.click();
  await until(() => document.activeElement?.dataset.rowId === target, 'Sequence jump did not focus its measured row');
  const targetBox = document.activeElement.getBoundingClientRect(), viewport = scroll.getBoundingClientRect();
  assert(targetBox.bottom > viewport.top && targetBox.top < viewport.bottom, `Sequence jump focused an offscreen row: ${JSON.stringify({target: {top: targetBox.top, bottom: targetBox.bottom}, viewport: {top: viewport.top, bottom: viewport.bottom}, scrollTop: scroll.scrollTop, scrollHeight: scroll.scrollHeight, following: !document.querySelector('.chat-follow')})}`);
  assert(document.querySelectorAll('.synth-sequence-column').length < 80, 'All sequence mounted 5k columns');
  nonoverlapping(scroll); isolated();
  return { ...await checkTracker(), heldArrivals: true, recycledDisclosure: true, pinnedFocus: true, sequenceTarget: target, mountedSequenceColumns: document.querySelectorAll('.synth-sequence-column').length };
}
