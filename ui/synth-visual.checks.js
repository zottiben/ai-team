// Computed production styles, not a second render of the design mockups.
export function inspectPatchVisuals() {
  const failures = [], bay = document.querySelector('.synth-patch-bay'), inspector = document.querySelector('.chat-context .synth-patch-inspector');
  if (!inspector) failures.push('Patch inspector has no retained sidebar');
  if (innerWidth > 1180 && inspector && document.querySelector('.chat-body').dataset.synthStacked === 'false') {
    const workspace = document.querySelector('.chat-workspace').getBoundingClientRect(), sidebar = inspector.closest('.chat-context').getBoundingClientRect(), rack = inspector.getBoundingClientRect();
    if (Math.abs(sidebar.top - workspace.top) > 1 || sidebar.width < 320 || Math.abs(sidebar.right - workspace.right) > 1 || rack.right > sidebar.right + 1) failures.push('Patch inspector escaped the shared Overview sidebar');
  }
  for (const module of bay?.querySelectorAll('.synth-patch-module') ?? []) {
    const kind = JSON.parse(module.dataset.patchId)[0], tone = module.dataset.tone;
    if (!tone || kind === 'draft' && tone !== 'night') failures.push('Patch module lost its role faceplate');
    const lcd = module.querySelector('.synth-lcd');
    if (!lcd || !getComputedStyle(lcd).fontFamily.includes('IBM Plex Mono')) failures.push('Recorded module facts are not in their mono LCD well');
  }
  if (failures.length) throw Error(failures.join('; '));
  return { retainedInspector: true, roleFaceplates: true, recordedReadouts: true };
}
export async function inspectTerminalVisuals() {
  await document.fonts.ready;
  const failures = [], screen = [...document.querySelectorAll('.synth-screen__well.terminal__host')].find(node => node.getClientRects().length);
  if (!screen) throw Error('Visible terminal screen missing');
  const before = screen.getBoundingClientRect();
  if (before.height < 320 || before.height > 562) failures.push('Terminal screen has no bounded independent height');
  const grid = screen.querySelector('.xterm-screen'), mount = screen.querySelector('.synth-screen__mount');
  if (!mount || !grid || grid.getBoundingClientRect().right > mount.getBoundingClientRect().right + 1 || grid.getBoundingClientRect().bottom > mount.getBoundingClientRect().bottom + 1) failures.push('Fitted terminal extends outside its padded screen');
  if (innerWidth > 1180 && document.querySelector('.chat-workspace[data-synth-tool="terminal"]')) {
    const rack = document.querySelector('.synth-rack').getBoundingClientRect(), workspace = document.querySelector('.chat-workspace').getBoundingClientRect();
    if (Math.abs(rack.top - workspace.top) > 1) failures.push('Process rack starts below the session header');
  }
  await new Promise(resolve => setTimeout(resolve, 350));
  if (Math.abs(screen.getBoundingClientRect().height - before.height) > 1) failures.push('Terminal fitting feeds back into its own size');
  if (failures.length) throw Error(failures.join('; '));
  return { boundedScreen: true, containedGrid: true, stableFit: true };
}
export function inspectEditorVisuals() {
  const failures = [];
  for (const name of ['.synth-editor__browser', '.synth-editor__centre', '.synth-minimap', '.synth-editor__monitor']) {
    const node = document.querySelector(name);
    if (!node?.getClientRects().length) failures.push(`${name} was removed from the responsive workspace`);
  }
  if ((document.querySelector('.synth-editor__centre')?.getBoundingClientRect().height ?? 0) < 320) failures.push('Editor code viewport is shorter than 320px');
  if (innerWidth > 1180) {
    const monitor = document.querySelector('.synth-editor__monitor')?.getBoundingClientRect();
    const workspace = document.querySelector('.chat-workspace')?.getBoundingClientRect();
    if (monitor && workspace && Math.abs(monitor.top - workspace.top) > 1) failures.push('Agent monitor starts below the session header');
  }
  const bar = document.querySelector('.synth-minimap__bar');
  if (!bar || bar.getBoundingClientRect().height > 4) failures.push('A short file stretches its minimap line into a solid block');
  for (const selector of ['.synth-editor__name', '.synth-editor__tab-key', '.synth-editor__status']) {
    const item = document.querySelector(selector);
    if (item && !getComputedStyle(item).fontFamily.includes('IBM Plex Mono')) failures.push(`${selector} does not use the specified mono font`);
  }
  const code = document.querySelector('.cm-content'), active = document.querySelector('.cm-activeLine');
  if (!code || !getComputedStyle(code).fontFamily.includes('IBM Plex Mono')) failures.push('Legacy font overrides the CodeMirror Synth theme');
  if (code && getComputedStyle(code).lineHeight !== '22px') failures.push('Code line height differs from the 22px spec');
  if (active) {
    const sample = document.createElement('span'); sample.hidden = true; sample.style.backgroundColor = 'var(--code-active-line)'; document.querySelector('.synth-editor').append(sample);
    const expected = getComputedStyle(sample).backgroundColor; sample.remove();
    if (getComputedStyle(active).backgroundColor !== expected) failures.push('Legacy active-line colour overrides the Synth theme');
  }
  if (document.documentElement.scrollWidth > innerWidth) failures.push('Editor expands the document horizontally');
  if (failures.length) throw Error(failures.join('; '));
  return { browser: true, monitor: true, minimap: true, boundedBars: true };
}
export async function inspectShellVisuals() {
  await document.fonts.ready;
  const failures = [];
  const check = (test, message) => { if (!test) failures.push(message); };
  const style = selector => {
    const element = document.querySelector(selector);
    if (!element) throw Error(`Missing visual owner: ${selector}`);
    return getComputedStyle(element);
  };
  for (const element of document.querySelectorAll('[hidden]')) {
    check(getComputedStyle(element).display === 'none', `Hidden ${element.className || element.tagName} is painted`);
  }
  const body = document.querySelector('.chat-body');
  if (body?.dataset.context === 'true' && body.dataset.synthStacked === 'false') {
    const panel = document.querySelector('#chat-context').getBoundingClientRect();
    const workspace = document.querySelector('.chat-workspace').getBoundingClientRect();
    check(Math.abs(panel.top - workspace.top) < 1, 'Right rack starts below the session header instead of beside it');
    check(Math.abs(panel.bottom - workspace.bottom) < 1, 'Right rack lost the workspace bottom');
  }
  const transcript = document.querySelector('.chat-transcript'), outcome = document.querySelector('.chat-outcome');
  if (transcript && outcome && !document.querySelector('.chat-conversation')?.hidden) {
    check(transcript.getBoundingClientRect().bottom <= outcome.getBoundingClientRect().top + 1, 'Transcript overlaps the outcome card at this viewport');
  }
  const title = document.querySelector('.synth-titlebar');
  check(!!title?.querySelector('.synth-preset'), 'Title bar is missing the session LCD selector');
  check(!!title?.querySelector('.notification-center'), 'Notifications are not in the title transport');
  check(style('.chat-header').backgroundImage === 'none', 'Chat header paints a faceplate instead of the chassis');
  check(style('.chat-context').backgroundColor === style('.chat-shell').backgroundColor, 'Context panel leaks the legacy surface');
  check(style('.chat-heading h1').whiteSpace !== 'nowrap', 'Long session titles cannot wrap');
  const lane = document.querySelector('.synth-tracker-lane');
  if (lane) {
    check(getComputedStyle(lane).borderRadius === '999px', 'Tracker lane is not a hardware pill');
    check(getComputedStyle(lane).backgroundColor !== 'rgba(0, 0, 0, 0)', 'Tracker lane is hollow instead of signal filled');
  }
  const slicePad = document.querySelector('.synth-slice-pad');
  if (slicePad?.getClientRects().length) {
    check(getComputedStyle(slicePad).fontFamily.includes('IBM Plex Mono') && getComputedStyle(slicePad).fontSize === '10px', 'Legacy form inheritance overrides slice-pattern typography');
    check(style('.chat-planning .plan-heading .button').borderRadius === '999px', 'Add slice is not a hardware key');
  }
  const runNumber = document.querySelector('.synth-turn__number');
  if (runNumber?.getClientRects().length) check(runNumber.getBoundingClientRect().height <= 40, 'Run-number LCD stretches to the full prompt height');
  if (failures.length) throw Error(failures.join('; '));
  return { hiddenOwners: document.querySelectorAll('[hidden]').length, titleTransport: true, scopedSurfaces: true, lanePills: lane ? true : null };
}
