import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

const css = readFileSync(join(import.meta.dirname, "tokens.css"), "utf8");
const blocks = [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]+)\}/g)];
const tokens = (body: string) => [...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(m => [m[1]!, m[2]!] as const);
const base = tokens(blocks[0]?.[2] ?? "");
function palette(theme: "dark" | "light") {
  const values = new Map([...base, ...theme === "light" ? tokens(blocks[1]?.[2] ?? "") : []]);
  const resolve = (name: string): string => {
    const value = values.get(name);
    if (!value) throw new Error(`Missing colour token ${name}`);
    return value.replace(/var\((--[\w-]+)\)/g, (_match, token: string) => resolve(token));
  };
  return resolve;
}
function colour(value: string): number[] {
  if (value.startsWith("#")) return [1, 3, 5].map(i => parseInt(value.slice(i, i + 2), 16));
  return (value.match(/[\d.]+/g) ?? []).map(Number);
}
function luminance(rgb: number[]) {
  return rgb.slice(0, 3).reduce((sum, value, i) => {
    const c = value / 255;
    return sum + (c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4) * [0.2126, 0.7152, 0.0722][i]!;
  }, 0);
}
function contrast(fg: string, bg: string) {
  const foreground = colour(fg), background = colour(bg), alpha = foreground[3] ?? 1;
  const blended = foreground.slice(0, 3).map((c, i) => c * alpha + background[i]! * (1 - alpha));
  const a = luminance(blended), b = luminance(background);
  return (Math.max(a, b) + .05) / (Math.min(a, b) + .05);
}
for (const theme of ["dark", "light"] as const) {
  const resolve = palette(theme);
  const check = (ink: string, surface: string, minimum = 4.5) => {
    const background = resolve(surface);
    const stops = background.startsWith("linear-gradient") ? background.match(/#[a-f\d]{6}/gi) ?? [] : [background];
    expect(stops.length).toBeGreaterThan(0);
    for (const stop of stops) expect(contrast(resolve(ink), stop), `${theme}: ${ink} on ${surface} (${stop})`).toBeGreaterThanOrEqual(minimum);
  };
  it(`${theme}: diff text and gutters remain readable over added, removed and selected rows`, () => {
    const surface = resolve("--code-surface");
    const over = (ink: string, alpha: number) => {
      const foreground = colour(resolve(ink)), background = colour(surface), opacity = (foreground[3] ?? 1) * alpha;
      return `rgb(${foreground.slice(0, 3).map((value, index) => value * opacity + background[index]! * (1 - opacity)).join(",")})`;
    };
    for (const background of [surface, over("--code-added-tint", 1), over("--git-deleted", .13), over("--sig-fix", .16)]) {
      for (const ink of ["--code-text", "--code-gutter"]) expect(contrast(resolve(ink), background), `${theme}: ${ink} on diff ${background}`).toBeGreaterThanOrEqual(4.5);
    }
  });
  it(`${theme}: body, secondary text and LCDs meet small-text contrast`, () => {
    for (const surface of ["--synth-floor", "--synth-void", "--synth-screen", "--plate-night", "--plate-midnight", "--plate-deck", "--synth-key", "--synth-key-hover"]) {
      for (const ink of ["--synth-text", "--synth-text-muted"]) check(ink, surface);
    }
    for (const ink of ["--synth-lcd-text", "--synth-lcd-text-alt", "--synth-lcd-label"]) check(ink, "--synth-screen");
  });
  it(`${theme}: shell labels and command/status labels remain readable`, () => {
    check("--synth-label", "--plate-midnight");
    check("--synth-accent", "--synth-key");
    for (const ink of ["--lane-think", "--lane-read", "--lane-edit", "--lane-bash", "--lane-mcp", "--st-working", "--st-attention"]) {
      check(ink, "--plate-midnight");
      check(ink, "--synth-command-pad");
    }
    check("--synth-focus", "--synth-floor", 3);
    check("--synth-focus", "--synth-key", 3);
  });
  it(`${theme}: code syntax and line numbers meet small-text contrast`, () => {
    for (const role of ["text", "punctuation", "keyword", "variable", "string", "number", "function", "type", "comment", "gutter", "gutter-active"]) {
      check(`--code-${role}`, "--code-surface");
    }
  });
  it(`${theme}: the terminal palette stays readable on the screen it paints on`, () => {
    // A terminal is almost entirely small text, so this is the surface where a
    // decorative colour costs legibility outright. `black` and `brightBlack` are left
    // out on purpose: in any ANSI palette they are the near-background shades programs
    // use for dimmed decoration, and holding them to text contrast would mean a
    // terminal that cannot dim anything.
    check("--synth-term-fg", "--synth-term-bg", 7);
    for (const ink of ["red", "green", "yellow", "blue", "magenta", "cyan", "white",
      "bright-red", "bright-green", "bright-yellow", "bright-blue", "bright-magenta", "bright-cyan", "bright-white"]) {
      check(`--synth-term-${ink}`, "--synth-term-bg");
    }
  });
  it(`${theme}: branded faceplates and primary/danger keys keep their own contrast`, () => {
    for (const surface of ["--plate-lavender", "--plate-ice", "--plate-flamingo", "--plate-yellow"]) {
      check("--vc-midnight", surface); check("--vc-text-muted", surface);
    }
    check("--synth-text-on-magenta", "--plate-magenta");
    check("--vc-midnight", "--synth-primary");
    check("--synth-text-on-danger", "--synth-danger");
  });
}
