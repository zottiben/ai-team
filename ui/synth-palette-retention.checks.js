// Actual retained CodeMirror/xterm owners on the offline tool fixture only.
export async function exercisePaletteRetention() {
  const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async test => { for (let n = 0; n < 100; n++) { const value = test(); if (value) return value; await wait(50); } throw Error('Retained palette fixture did not become ready'); };
  const chord = key => document.dispatchEvent(new KeyboardEvent('keydown', { key, metaKey: true, bubbles: true, cancelable: true }));
  const entry = [...document.scripts].find(script => script.src.includes('/src/synth/shell-preview.tsx'));
  if (!entry) throw Error('Exact fixture entry missing');
  const { tools, fixtureCommands } = await import(entry.src);
  if (!tools) throw Error('Use tools=true&tool-input=true');
  const { setSynthPalette } = await import('/src/ui-preferences.ts');
  const { SYNTH_PALETTES } = await import('/src/synth-palettes.ts');
  const draft = await until(() => document.querySelector('textarea[aria-label="Message"]'));
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(draft, 'Retain palette-switch draft');
  draft.dispatchEvent(new Event('input', { bubbles: true }));
  chord('e');
  (await until(() => [...document.querySelectorAll('[role="treeitem"]')].find(el => el.textContent.includes('calc.ts')))).click();
  const content = await until(() => document.querySelector('[aria-label="Source calc.ts"]'));
  const viewUrl = performance.getEntriesByType('resource').find(entry => entry.name.includes('/@codemirror_view.js'))?.name;
  const { EditorView } = await import(viewUrl);
  const view = EditorView.findFromDOM(content), original = view.state.doc.toString();
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: 'export const total = 123;\n' } });
  const doc = view.state.doc, selection = view.state.selection.main;
  const editorColours = [];
  for (const { id } of SYNTH_PALETTES) {
    setSynthPalette(id); await wait(80);
    if (EditorView.findFromDOM(content) !== view || view.state.doc !== doc || !view.state.selection.main.eq(selection)) throw Error('Palette replaced editor document/selection owner');
    editorColours.push(getComputedStyle(view.dom).backgroundColor);
  }
  if (new Set(editorColours).size !== 6) throw Error('CodeMirror did not repaint all palettes');
  // Real undo on the retained instance: no save or file write was requested.
  const commandUrl = performance.getEntriesByType('resource').find(entry => entry.name.includes('/@codemirror_commands.js'))?.name;
  if (!commandUrl || !(await import(commandUrl)).undo(view) || view.state.doc.toString() !== original) throw Error('Palette lost editor undo history');
  await import('/src/synth/views/Terminal/Screen.tsx');
  const termUrl = performance.getEntriesByType('resource').find(entry => entry.name.includes('/@xterm_xterm.js'))?.name;
  if (!termUrl) throw Error('Loaded xterm module unavailable');
  const { Terminal } = await import(termUrl);
  const open = Terminal.prototype.open;
  let instance;
  Terminal.prototype.open = function (...args) { instance = this; return open.apply(this, args); };
  let term;
  try { chord('`'); term = await until(() => document.querySelector('.xterm')); }
  finally { Terminal.prototype.open = open; }
  if (!instance || instance.element !== term) throw Error('Real terminal instance not captured');
  await until(() => document.querySelector('.xterm-screen')?.getBoundingClientRect().height > 0);
  await until(() => document.querySelector('.synth-process-rack')?.textContent.includes('Renderer:') || document.body.textContent.includes('Renderer:'));
  await wait(100);
  // WebGL intentionally replaces its display:none glyph-cache canvas on recolour.
  // Its visible renderer canvases and input owner must remain mounted.
  const canvases = [...term.querySelectorAll('canvas')].filter(canvas => getComputedStyle(canvas).display !== 'none');
  const terminalInput = term.querySelector('.xterm-helper-textarea');
  const buffer = instance.buffer.active, firstLine = buffer.getLine(0)?.translateToString();
  const { terminalTheme } = await import('/src/synth/terminal-theme.ts');
  const backgrounds = [];
  for (const { id } of SYNTH_PALETTES) {
    setSynthPalette(id); await wait(180);
    if (document.querySelector('.xterm') !== term) throw Error('Palette replaced xterm');
    if (canvases.some(canvas => !canvas.isConnected) || term.querySelector('.xterm-helper-textarea') !== terminalInput) throw Error('Palette replaced terminal renderer/input owner');
    if (instance.buffer.active !== buffer || buffer.getLine(0)?.translateToString() !== firstLine) throw Error('Palette reset the terminal buffer');
    const expected = terminalTheme();
    for (const [key, value] of Object.entries(expected)) if (instance.options.theme[key] !== value) throw Error(`Stale terminal ${key} in ${id}`);
    backgrounds.push(instance.options.theme.background);
  }
  if (new Set(backgrounds).size !== 6) throw Error(`Retained terminal did not repaint: ${backgrounds}`);
  if (document.querySelector('textarea[aria-label="Message"]') !== draft || draft.value !== 'Retain palette-switch draft') throw Error('Palette lost the composer draft');
  const observations = new Set(['/api/terminals/7/resize', '/api/lsp/diagnostics']);
  if (fixtureCommands.some(command => !observations.has(command.path)) || tools.commands.some(command => !command.path.endsWith('/resize'))) throw Error('Palette saved, sent or executed work');
  return { editorColours, terminalColours: backgrounds, sameEditor: true, undoRetained: true, sameTerminal: true, sameCanvases: true, draftRetained: true, commands: 0 };
}
