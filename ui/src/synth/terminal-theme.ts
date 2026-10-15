/**
 * The xterm theme, read out of the token block.
 *
 * xterm paints glyphs into a canvas and cannot resolve a CSS variable, so the colours
 * have to be handed to it as values. Resolving them from the tokens rather than writing
 * them here is what keeps §19's "no colours exist outside the token block" true of the
 * one surface that is not styled by CSS at all - and it is why the pane repaints when
 * the window goes light instead of staying a dark rectangle in a pale app.
 */

const TOKENS = {
  background: "--synth-term-bg",
  foreground: "--synth-term-fg",
  cursor: "--synth-term-cursor",
  cursorAccent: "--synth-term-cursor-accent",
  selectionBackground: "--synth-term-selection",
  black: "--synth-term-black",
  red: "--synth-term-red",
  green: "--synth-term-green",
  yellow: "--synth-term-yellow",
  blue: "--synth-term-blue",
  magenta: "--synth-term-magenta",
  cyan: "--synth-term-cyan",
  white: "--synth-term-white",
  brightBlack: "--synth-term-bright-black",
  brightRed: "--synth-term-bright-red",
  brightGreen: "--synth-term-bright-green",
  brightYellow: "--synth-term-bright-yellow",
  brightBlue: "--synth-term-bright-blue",
  brightMagenta: "--synth-term-bright-magenta",
  brightCyan: "--synth-term-bright-cyan",
  brightWhite: "--synth-term-bright-white",
} as const;

/**
 * Where a token has not been applied - the legacy skin, or a test with no stylesheet -
 * the legacy surface tokens are used instead, so the pane is still readable rather than
 * black on black. Never a literal from the spec: a fallback that hardcoded the palette
 * would be the colour outside the token block this exists to prevent.
 */
const FALLBACK: Partial<Record<keyof typeof TOKENS, string>> = {
  background: "--surface-canvas-default",
  foreground: "--text-primary-default",
  cursor: "--accent-action-default",
  selectionBackground: "--accent-action-muted",
};

export type TerminalTheme = Partial<Record<keyof typeof TOKENS, string>>;

export function terminalTheme(root: Element = document.documentElement): TerminalTheme {
  const styles = getComputedStyle(root);
  const read = (name: string) => styles.getPropertyValue(name).trim();
  const theme: TerminalTheme = {};
  for (const [key, token] of Object.entries(TOKENS) as [keyof typeof TOKENS, string][]) {
    const value = read(token) || (FALLBACK[key] ? read(FALLBACK[key]) : "");
    if (value) theme[key] = value;
  }
  return theme;
}

/**
 * Watch for a theme change and hand back the new values.
 *
 * Light and dark are an attribute on the root element, and so is the skin itself, so a
 * pane that resolved its colours once at mount goes stale the moment either changes.
 */
export function onThemeChange(listener: (theme: TerminalTheme) => void): () => void {
  if (typeof MutationObserver === "undefined") return () => {};
  const root = document.documentElement;
  const observer = new MutationObserver(() => listener(terminalTheme(root)));
  observer.observe(root, { attributes: true, attributeFilter: ["data-theme", "data-synth"] });
  return () => observer.disconnect();
}
