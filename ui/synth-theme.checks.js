/** Real CSS/font checks for the isolated fixtures only (Chromium or WKWebView). */
export async function checkTheme(theme = document.documentElement.dataset.theme) {
  if (!['/synth-preview.html', '/synth-shell-preview.html'].includes(location.pathname)) throw new Error('Use an isolated Synth fixture');
  await document.fonts.ready;
  const root = document.documentElement;
  if (root.dataset.theme !== theme) throw new Error('Theme did not change');
  const host = document.querySelector('.synth-ui, .synth-titlebar');
  if (!host) throw new Error('Synth surface missing');
  const styles = getComputedStyle(host);
  const expectedScheme = getComputedStyle(root).getPropertyValue('--synth-color-scheme').trim();
  if (expectedScheme !== theme) throw new Error('Theme tokens are not active');
  if (!styles.fontFamily.includes('Figtree')) throw new Error('Sans font not wired');
  // Explicitly load the less common faces too, not just the ASCII upright seen on entry.
  await Promise.all([
    document.fonts.load('300 26px Figtree', 'Résumé Łódź'),
    document.fonts.load('italic 400 14px Figtree', 'Résumé Łódź'),
    document.fonts.load('400 12px "IBM Plex Mono"', 'Å Ł λ Ж Ắ Ѫ'),
    document.fonts.load('600 12px "IBM Plex Mono"', 'Å Ł λ Ж Ắ Ѫ'),
  ]);
  const loaded = [...document.fonts].filter(font => ['Figtree', 'IBM Plex Mono', '"IBM Plex Mono"'].includes(font.family) && font.status === 'loaded');
  if (loaded.length < 4) throw new Error('Bundled fonts did not decode');
  const resources = performance.getEntriesByType('resource');
  const remote = resources.filter(entry => /^https?:/.test(entry.name) && new URL(entry.name).origin !== location.origin);
  if (remote.length) throw new Error(`Fixture made external requests: ${remote.map(entry => entry.name).join(', ')}`);
  if (root.scrollWidth > root.clientWidth) throw new Error(`Theme overflow: ${root.scrollWidth} > ${root.clientWidth}`);
  const preset = document.querySelector('.synth-preset__value');
  if (preset) {
    preset.click();
    await new Promise(resolve => setTimeout(resolve, 50));
    const popover = document.querySelector('.synth-preset-popover');
    if (!popover || popover.parentElement !== document.body) throw new Error('Expected a body portal');
    const popup = getComputedStyle(popover);
    if (popup.colorScheme !== theme) throw new Error('Portal lost the native colour scheme');
    const screen = getComputedStyle(document.querySelector('.synth-preset'));
    if (popup.backgroundColor !== screen.backgroundColor) throw new Error('Portal lost the screen palette');
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    if (document.activeElement !== preset) throw new Error('Preset did not recover keyboard focus');
    if (preset.matches(':focus-visible')) {
      const reference = document.createElement('span');
      reference.style.color = 'var(--synth-focus)';
      preset.append(reference);
      const expected = getComputedStyle(reference).color;
      reference.remove();
      if (getComputedStyle(preset).outlineColor !== expected) throw new Error('Preset focus ring lost the Synth palette');
    }
  }
  const more = document.querySelector('.app-rail button[aria-label="More tools"]');
  if (more) {
    more.click();
    await new Promise(resolve => setTimeout(resolve, 50));
    const menu = document.querySelector('.synth-more-popover');
    if (!menu || getComputedStyle(menu).colorScheme !== theme || !getComputedStyle(menu).fontFamily.includes('Figtree')) throw new Error('More portal lost its theme');
    if (![...menu.querySelectorAll('button')].some(button => button.textContent.trim() === 'Schedule')) throw new Error('Schedule missing from More');
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  }
  return { theme, width: root.clientWidth, loadedFaces: loaded.length, externalRequests: remote.length, background: styles.backgroundColor, foreground: styles.color };
}

export async function exerciseThemes() {
  if (location.pathname !== '/synth-preview.html') throw new Error('Use the isolated primitive fixture');
  const until = performance.now() + 10000;
  while (!document.querySelector('[role="slider"]')) {
    if (performance.now() > until) throw new Error('Primitive fixture did not become ready');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  const knob = document.querySelector('[role="slider"]');
  const value = knob.getAttribute('aria-valuenow');
  const results = [];
  for (const theme of ['light', 'dark']) {
    const label = theme === 'light' ? 'Light' : 'Dark';
    [...document.querySelectorAll('.synth-fixture-themes button')].find(button => button.textContent === label).click();
    await new Promise(resolve => setTimeout(resolve, 100));
    results.push(await checkTheme(theme));
    if (document.querySelector('[role="slider"]') !== knob || knob.getAttribute('aria-valuenow') !== value) throw new Error('Theme switch discarded the control');
  }
  return results;
}
