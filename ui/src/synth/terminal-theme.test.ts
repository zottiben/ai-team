import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

import { terminalTheme } from "./terminal-theme";

const here = import.meta.dirname;
const css = readFileSync(join(here, "tokens.css"), "utf8");
const blocks = [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]+)\}/g)];
const tokens = (body: string) => new Map([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1]!, m[2]!.trim()]));
const dark = tokens(blocks.find((block) => block[1]?.trim() === '[data-synth="true"]')?.[2] ?? "");
const light = tokens(blocks.find((block) => block[1]?.includes('[data-theme="light"]'))?.[2] ?? "");

const ANSI = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "bright-black", "bright-red", "bright-green", "bright-yellow", "bright-blue", "bright-magenta", "bright-cyan", "bright-white",
];

it("gives the terminal a full palette in both themes, defined only in the token block", () => {
  // xterm is handed values rather than variables, so a missing one is a glyph painted
  // in whatever xterm's own default happens to be - which is not this app's palette.
  for (const name of [...ANSI.map((colour) => `--synth-term-${colour}`), "--synth-term-bg", "--synth-term-fg", "--synth-term-cursor", "--synth-term-selection"]) {
    expect(dark.get(name), `dark ${name}`).toBeDefined();
    expect(light.get(name), `light ${name}`).toBeDefined();
    expect(light.get(name), `${name} must differ between themes`).not.toBe(dark.get(name));
  }
});

it("resolves the theme it is given rather than a palette of its own", () => {
  const root = document.createElement("div");
  root.style.setProperty("--synth-term-bg", "#0a0119");
  root.style.setProperty("--synth-term-fg", "#e0c3ff");
  root.style.setProperty("--synth-term-bright-white", "#ffffff");
  document.body.append(root);
  const theme = terminalTheme(root);
  expect(theme.background).toBe("#0a0119");
  expect(theme.foreground).toBe("#e0c3ff");
  expect(theme.brightWhite).toBe("#ffffff");
  // Nothing is invented for a token that has not been applied.
  expect(theme.red).toBeUndefined();
  root.remove();
});

it("keeps every terminal colour out of the components", () => {
  // §19: no colours exist outside the token block. A literal here is a pane that stays
  // dark when the window goes light, and there is no browser on the machine this is
  // built on for anybody to notice.
  const dir = join(here, "views", "Terminal");
  for (const name of readdirSync(dir)) {
    if (name.endsWith(".test.tsx") || name.endsWith(".test.ts")) continue;
    const source = readFileSync(join(dir, name), "utf8");
    expect(source.match(/#[0-9a-fA-F]{3,8}\b/g) ?? [], `${name} hard-codes a colour`).toEqual([]);
    expect(source.match(/\brgba?\(/g) ?? [], `${name} hard-codes a colour`).toEqual([]);
  }
  const theme = readFileSync(join(here, "terminal-theme.ts"), "utf8");
  expect(theme.match(/#[0-9a-fA-F]{3,8}\b/g) ?? []).toEqual([]);
});
