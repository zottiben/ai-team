// Run against the network-disabled real App fixture, never an installed session.
export async function exercisePaletteSettingsPage() {
  const end = performance.now() + 10000;
  while (!document.querySelector('button[aria-label="Settings"]') && performance.now() < end) await new Promise(resolve => setTimeout(resolve, 50));
  document.querySelector('button[aria-label="Settings"]').click();
  while (!document.querySelector('.synth-palettes') && performance.now() < end) await new Promise(resolve => setTimeout(resolve, 50));
  await document.fonts.ready;
  return exercisePaletteSettings();
}

export async function checkSettingsScroll() {
  const wait = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const main = document.querySelector('.chat-main > .main');
  const settings = main?.querySelector('.settings');
  if (!settings) throw Error('Open fixture Settings before checking scroll');
  const shell = document.querySelector('.chat-shell');
  window.scrollTo(0, 0);
  const top = shell.getBoundingClientRect().top;
  main.scrollTop = main.scrollHeight;
  await wait();
  window.scrollTo(0, document.documentElement.scrollHeight);
  await wait();
  const excess = document.documentElement.scrollHeight - innerHeight;
  if (excess > 1 || window.scrollY !== 0 || shell.getBoundingClientRect().top !== top) {
    throw Error(`Settings escaped its scroll owner: document excess ${excess}px, window scroll ${window.scrollY}px`);
  }
  const last = settings.lastElementChild.getBoundingClientRect(), box = main.getBoundingClientRect();
  if (last.bottom > box.bottom + 1 || last.bottom < box.bottom - 50) throw Error('Settings bottom is unreachable or has a blank scroll tail');
  main.scrollTop = 0;
  await wait();
  if (settings.getBoundingClientRect().top < box.top - 1) throw Error('Settings cannot return to its heading');
  return { excess, windowScroll: window.scrollY, contentHeight: main.scrollHeight, viewportHeight: main.clientHeight };
}

export async function exercisePaletteSettings() {
  const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
  const group = document.querySelector('.synth-palettes');
  if (!group) throw Error('Open fixture Settings with Synth enabled');
  const preview = document.querySelector('.synth-component-preview');
  const entry = [...document.scripts].find(script => script.src.includes('/src/synth/shell-preview.tsx'));
  if (!entry) throw Error('Exact fixture entry missing');
  const { fixtureCommands } = await import(entry.src);
  const { initializeSynth, setSynth } = await import('/src/ui-preferences.ts');
  const theme = localStorage.getItem('ai-team.theme');
  const root = document.documentElement;
  const mode = root.dataset.theme;
  const inputs = [...group.querySelectorAll('input[type="radio"]')];
  if (inputs.length !== 6 || /#[a-f0-9]{3,8}\b|\brgba?\(/i.test(group.textContent)) throw Error('Missing swatches or exposed colour values');
  const manual = [...document.querySelectorAll('summary')].find(el => el.textContent === 'Manual token fallback');
  manual.click();
  const draft = document.querySelector('input[aria-label="figma token"]');
  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setValue.call(draft, 'fixture-only-unsaved'); draft.dispatchEvent(new Event('input', { bubbles: true }));
  if (document.documentElement.scrollWidth > innerWidth + 1 || group.scrollWidth > group.clientWidth + 1) throw Error('Palette cards overflow the page');
  const samples = [...group.querySelectorAll('.synth-palette__sample')];
  const backgrounds = samples.map(el => getComputedStyle(el).backgroundImage);
  if (new Set(backgrounds).size !== 6) throw Error('Swatches do not show their own palette');
  const results = [];
  // Select away from the implicit default first; clicking an already-checked native
  // radio correctly emits no change event and need not create a storage entry.
  inputs[1].click(); await wait(80);
  for (const input of inputs) {
    input.closest('label').scrollIntoView({ block: 'center' }); input.click(); await wait(80);
    if (root.dataset.synthPalette !== input.value || localStorage.getItem('ui.synth.palette') !== input.value || !input.checked) throw Error('Selection did not persist');
    if (root.dataset.theme !== mode || localStorage.getItem('ai-team.theme') !== theme) throw Error('Palette changed the colour mode');
    if (document.querySelector('input[aria-label="figma token"]') !== draft || draft.value !== 'fixture-only-unsaved') throw Error('Palette replaced an unsaved Settings field');
    const style = getComputedStyle(root);
    const floor = style.getPropertyValue('--synth-floor').trim();
    const paint = getComputedStyle(document.querySelector('.chat-main')).backgroundColor;
    const swatchFloor = getComputedStyle(samples[inputs.indexOf(input)]).getPropertyValue('--synth-floor').trim();
    if (floor !== swatchFloor) throw Error('Preview and whole-app palette differ');
    if (samples.some((sample, i) => getComputedStyle(sample).backgroundImage !== backgrounds[i])) throw Error('Selecting a palette recoloured the other swatches');
    initializeSynth();
    if (root.dataset.synthPalette !== input.value) throw Error('Startup lost the saved palette');
    preview.open = false; const compact = await checkSettingsScroll();
    preview.open = true; await wait(150); const expanded = await checkSettingsScroll();
    if (expanded.contentHeight <= compact.contentHeight + 500) throw Error('The original long-preview regression was not exercised');
    results.push({ palette: input.value, mode, paint, compact, expanded });
  }
  preview.open = false;
  const selected = root.dataset.synthPalette;
  setSynth(false); await wait(100);
  if (document.querySelector('.synth-palettes')) throw Error('Synth picker leaked into legacy');
  const legacy = getComputedStyle(document.querySelector('.settings__group')).backgroundImage;
  if (legacy !== 'none') throw Error('Palette changed legacy styling');
  await checkSettingsScroll();
  setSynth(true); await wait(150);
  if (root.dataset.synthPalette !== selected || document.querySelector('input[aria-label="figma token"]') !== draft || draft.value !== 'fixture-only-unsaved') throw Error('Skin toggle lost preference or unsaved Settings field');
  if (fixtureCommands.length) throw Error('Appearance change issued a backend command');
  return results;
}
