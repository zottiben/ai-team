const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
const assert = (test, message) => { if (!test) throw new Error(message); };
async function until(test, message) { for (let i = 0; i < 120; i++) { const value = test(); if (value) return value; await wait(50); } throw new Error(message); }
const tab = name => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent === name);
function cableGeometry(bay, jack) {
  assert(document.visibilityState === 'visible', 'Viewport became hidden during socket checks; visible verification blocked');
  const svg = bay.querySelector('.synth-patch-cables');
  const clip = { left: 0, top: 0, right: innerWidth, bottom: innerHeight };
  for (let parent = svg.parentElement; parent; parent = parent.parentElement) {
    const style = getComputedStyle(parent), rect = parent.getBoundingClientRect();
    const sx = parent.offsetWidth ? rect.width / parent.offsetWidth : 1, sy = parent.offsetHeight ? rect.height / parent.offsetHeight : 1;
    const left = rect.left + parent.clientLeft * sx, top = rect.top + parent.clientTop * sy;
    if (/auto|scroll|hidden|clip/.test(style.overflowX)) { clip.left = Math.max(clip.left, left); clip.right = Math.min(clip.right, left + parent.clientWidth * sx); }
    if (/auto|scroll|hidden|clip/.test(style.overflowY)) { clip.top = Math.max(clip.top, top); clip.bottom = Math.min(clip.bottom, top + parent.clientHeight * sy); }
  }
  const centre = node => { const r = node.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; };
  const sockets = new Map([...bay.querySelectorAll('[data-patch-id]')].map(module => [module.dataset.patchId, centre(module.querySelector('.synth-jack > span'))]));
  const errors = [];
  for (const plug of svg.querySelectorAll('.synth-cable__plug')) {
    const p = centre(plug), distance = Math.min(...[...sockets.values()].map(s => Math.hypot(p.x - s.x, p.y - s.y)));
    if (distance > 1) errors.push(`Cable plug misses its socket by ${distance.toFixed(1)}px`);
  }
  const links = [...svg.querySelectorAll('[data-patch-from]')];
  if (links.length !== 6) errors.push('Recorded endpoints are not identified');
  for (const link of links) {
    [...link.querySelectorAll('.synth-cable__plug')].forEach((plug, index) => {
      const socket = sockets.get(index === 0 ? link.dataset.patchFrom : link.dataset.patchTo), p = centre(plug);
      if (!socket || Math.hypot(p.x - socket.x, p.y - socket.y) > 1) errors.push('Cable points to the wrong recorded module');
    });
  }
  const bounds = svg.getBoundingClientRect();
  for (const path of svg.querySelectorAll('.synth-cable path')) {
    const box = path.getBoundingClientRect();
    if (box.left - 4 < bounds.left || box.right + 4 > bounds.right || box.top - 4 < bounds.top || box.bottom + 4 > bounds.bottom) errors.push('Cable stroke is clipped by its SVG bounds');
  }
  const p = centre(jack.querySelector('.synth-jack__socket'));
  assert(p.x > clip.left + 12 && p.x < clip.right - 12 && p.y > clip.top + 12 && p.y < clip.bottom - 12, 'Inspected socket is not fully visible');
  // Hit testing only: this changes neither paint order nor geometry, and is restored
  // before any interaction. A transparent, noninteractive SVG is otherwise skipped.
  const pointerEvents = svg.style.pointerEvents;
  try {
    svg.style.pointerEvents = 'all';
    const top = document.elementFromPoint(p.x, p.y);
    if (!svg.contains(top)) errors.push(`Cable layer is painted behind ${top?.tagName}.${top?.getAttribute('class')} at ${p.x.toFixed(1)},${p.y.toFixed(1)} (clip ${JSON.stringify(clip)})`);
  } finally { svg.style.pointerEvents = pointerEvents; }
  assert(errors.length === 0, [...new Set(errors)].join('; '));
  assert(getComputedStyle(svg).pointerEvents === 'none', 'Cable decoration intercepted inspection controls');
  assert(document.elementFromPoint(p.x, p.y)?.closest('.synth-jack') === jack, 'The inspected socket cannot receive input');
}
async function checkPatchGeometry(bay) {
  (await import('/synth-visual.checks.js')).inspectPatchVisuals();
  bay.querySelector('.synth-patch-canvas').scrollIntoView({ block: 'start', behavior: 'instant' });
  await until(() => bay.querySelectorAll('.synth-cable__plug').length === 12, 'All recorded cable ends unavailable');
  await document.fonts.ready;
  const width = [...document.querySelectorAll('.synth-patch-inspector label')].find(label => label.textContent.startsWith('Canvas width'))?.querySelector('select');
  assert(width, 'Canvas width control unavailable'); const saved = width.value;
  const widths = ['0.8', '1', '1.25', '1.5'];
  try {
    for (const value of widths) {
      width.value = value; width.dispatchEvent(new Event('change', { bubbles: true })); await wait(80);
      assert(bay.querySelector('.synth-plate').getBoundingClientRect().width <= bay.getBoundingClientRect().width + 1, 'Patch plate expanded beyond the retained panel instead of scrolling its canvas');
      for (const jack of bay.querySelectorAll('.synth-jack')) {
        jack.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        cableGeometry(bay, jack);
      }
    }
  } finally { width.value = saved; width.dispatchEvent(new Event('change', { bubbles: true })); }
  return { widths: widths.map(Number), connectedEnds: 12, foregroundLayer: true, unclippedStrokes: true, scrolledSockets: true };
}
export async function exercisePatchGeometry() {
  assert(!document.hidden, 'Visible viewport required for socket geometry');
  (await until(() => tab('Overview'), 'Overview tab unavailable')).click();
  const bay = await until(() => document.querySelector('.synth-patch-bay'), 'Team patch bay unavailable');
  const geometry = await checkPatchGeometry(bay);
  const api = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/'));
  assert(api.length === 0, 'Jack geometry reached a real API');
  return { ...geometry, apiRequests: api.length };
}
export async function exercisePatchBay() {
  assert(!document.hidden, 'Visible viewport required; hidden native acceptance is blocked');
  const overview = await until(() => tab('Overview'), 'Overview tab unavailable'); overview.click();
  const bay = await until(() => document.querySelector('.synth-patch-bay'), 'Team patch bay unavailable');
  assert(document.querySelectorAll('.synth-patch-bay').length === 1, 'Duplicate fixture or patch-bay owner');
  assert(document.querySelector('.chat-body').dataset.context === 'true' && !document.querySelector('.chat-conversation').hidden, 'Team Overview hid the conversation instead of using its sidebar');
  assert(document.querySelector('.chat-conversation').contains(bay), 'Patch bay is not beside the conversation');
  const button = await until(() => [...bay.querySelectorAll('button')].find(node => node.textContent === 'frontend · node #3'), 'Exact member unavailable');
  button.scrollIntoView({ block: 'center', inline: 'center' }); button.focus(); button.click();
  const inspector = await until(() => document.querySelector('.chat-context [aria-label="Patch inspector"]'), 'Sidebar inspector unavailable');
  await until(() => inspector.textContent.includes('Latest reported prompt/context: 1400'), 'Wrong seat context');
  assert(inspector.textContent.includes('estimated after: 1000'), 'Explicit compaction estimate missing');
  assert(bay.querySelectorAll('[data-patch-id]').length === 5, 'Wrong module cardinality');
  const geometry = await checkPatchGeometry(bay);
  const selected = button;
  document.querySelector('.chat-context-heading > button').click();
  await until(() => document.querySelector('[aria-label="Message"]').getClientRects().length > 0, 'Closing team view did not reveal retained composer');
  overview.click(); await until(() => selected.getClientRects().length > 0, 'Retained module not displayed');
  assert(selected.getAttribute('aria-pressed') === 'true', 'Selection lost across panel visibility');
  const { setSynth } = await import('/src/ui-preferences.ts');
  setSynth(false); await wait(50); setSynth(true); await until(() => selected.getClientRects().length, 'Skin return failed');
  assert(bay === document.querySelector('.synth-patch-bay') && selected.getAttribute('aria-pressed') === 'true', 'Skin switch replaced evidence owner');
  const src = [...document.scripts].find(script => script.src.includes('/src/synth/shell-preview.tsx'))?.src;
  assert(src, 'Exact fixture entry missing'); const fixture = await import(src);
  fixture.advanceTeamTelemetry();
  await until(() => inspector.textContent.includes('Latest reported prompt/context: 1600'), 'Live receipt never reached its exact inspector');
  await until(() => document.querySelector('.synth-title-metrics')?.textContent.includes('Prompt 1600'), 'Title did not sample the same receipt');
  assert(document.querySelector('.synth-title-metrics').textContent.includes('Tokens 735'), 'Live cumulative observation changed completed accounting');
  fixture.removeTeamMember();
  await until(() => !document.querySelector('[aria-label="Patch inspector"]'), 'Missing selected member borrowed another inspector');
  assert(document.querySelector('.synth-patch-inspector').textContent.includes('missing selection is not replaced'), 'Missing exact selection not disclosed');
  assert(fixture.fixtureCommands.length === 0, 'Evidence inspection attempted a backend command');
  assert(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, 'Patch bay overflowed the page');
  const api = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/'));
  assert(api.length === 0, 'A real API request escaped the fixture');
  return { ...geometry, modulesBefore: 5, relationships: 6, retainedSelection: true, liveContext: 1600, canonicalTokens: 735, missingSelection: true, commands: 0, apiRequests: api.length };
}
