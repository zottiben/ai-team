import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

const css = readFileSync(join(import.meta.dirname, "tokens.css"), "utf8");
const blocks = [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]+)\}/g)];
const base = blocks.find(block => block[1]?.trim() === '[data-synth="true"]');
const light = blocks.find(block => block[1]?.includes('[data-theme="light"]'));
const tokens = (block: string) => new Map([...block.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(m => [m[1]!, m[2]!]));

it("adds a scoped light palette for both the app root and nested preview without redefining the brand colours", () => {
  expect(light, "Synth needs a light theme, not a dark island in the light app").toBeDefined();
  expect(light?.[1]).toContain('[data-theme="light"][data-synth="true"]');
  expect(light?.[1]).toContain('[data-theme="light"] [data-synth="true"]');
  const darkTokens = tokens(base?.[2] ?? ""), lightTokens = tokens(light?.[2] ?? "");
  for (const name of lightTokens.keys()) expect(darkTokens.has(name), name).toBe(true);
  expect([...lightTokens.keys()].filter(name => name.startsWith("--vc-"))).toEqual([]);
  for (const name of ["--synth-floor", "--synth-panel", "--synth-screen", "--synth-text", "--synth-text-muted", "--synth-focus", "--lane-think", "--lane-edit", "--st-working", "--plate-night"]) {
    expect(lightTokens.get(name), name).toBeDefined();
    expect(lightTokens.get(name), name).not.toBe(darkTokens.get(name));
  }
});

it("uses the theme for native controls and separates surface ink from neon decoration", () => {
  const primitives = readFileSync(join(import.meta.dirname, "primitives.css"), "utf8");
  const shell = readFileSync(join(import.meta.dirname, "views/Shell/shell.css"), "utf8");
  expect(primitives).not.toContain("color-scheme: dark;");
  expect(primitives).toContain("color-scheme: var(--synth-color-scheme)");
  expect(primitives).toContain("outline: 2px solid var(--synth-focus)");
  expect(shell).toContain("color: var(--synth-label)");
  expect(shell).toContain("color: var(--synth-accent)");
});
