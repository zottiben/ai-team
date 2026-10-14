import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

import { expect, it } from "vitest";

/**
 * The rule that makes a theme switch work at all.
 *
 * Every surface resolves a semantic token; the palette lives in exactly one file. A
 * single raw colour in a component is a component that stays dark when the window goes
 * light, and that is the kind of bug nobody notices until somebody screenshots it.
 *
 * Enforced here rather than by review, because there is no browser on the machine this
 * is built on - the author cannot simply look.
 */
const SRC = join(import.meta.dirname, ".");
const COLOUR = /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|oklch|oklab|hwb|color)\s*\(|(?:color|background|stroke|fill)\s*:\s*(?:white|black|red|blue|green|yellow|pink|purple|orange|grey|gray)\b/;
const TOKEN_FILES = [join(SRC, "tokens.css"), join(SRC, "synth/tokens.css")];

function sourceFiles(dir = SRC): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? sourceFiles(path) : [path];
  });
}
function styleSheets(): string[] {
  return sourceFiles().filter(path => path.endsWith(".css") && !TOKEN_FILES.includes(path));
}
function synthSources(): string[] {
  return sourceFiles(join(SRC, "synth")).filter(path => /\.tsx?$/.test(path) && !path.includes(".test."));
}

it("only the token layer names a colour", () => {
  for (const path of [...styleSheets(), ...synthSources()]) {
    const offending = readFileSync(path, "utf8")
      .split("\n")
      .map((line, index) => ({ line: line.trim(), number: index + 1 }))
      .filter(({ line }) => !line.startsWith("*") && !line.startsWith("/*"))
      .filter(({ line }) => COLOUR.test(line));

    expect(
      offending,
      `${path} names a colour directly; add a token to tokens.css instead`,
    ).toEqual([]);
  }
});

it("synth tokens cannot leak into or redefine the legacy palette", () => {
  const css = readFileSync(join(SRC, "synth/tokens.css"), "utf8");
  const legacy = new Set([...readFileSync(join(SRC, "tokens.css"), "utf8").matchAll(/(--[a-z0-9-]+):/g)].map(m => m[1]));
  const definitions = [...css.matchAll(/(--[a-z0-9-]+):/g)].map(m => m[1]);
  expect(css.replace(/\/\*[\s\S]*?\*\//g, "").trim().startsWith('[data-synth="true"] {')).toBe(true);
  expect((css.match(/{/g) ?? []).length).toBe(1);
  expect(definitions.filter(token => legacy.has(token))).toEqual([]);
  expect(definitions.length).toBeGreaterThan(100);
});

it("every semantic token is defined in both themes", () => {
  // A token that exists in one theme only is a surface that loses its colour halfway
  // through a switch - and it renders as "unset", which usually looks like black.
  const css = readFileSync(join(SRC, "tokens.css"), "utf8");
  const block = (selector: string) => {
    const start = css.indexOf(selector);
    expect(start, `${selector} block is missing`).toBeGreaterThan(-1);
    const open = css.indexOf("{", start);
    const close = css.indexOf("}", open);
    return new Set([...css.slice(open, close).matchAll(/(--[a-z0-9-]+):/g)].map((m) => m[1]));
  };

  const dark = block('[data-theme="dark"]');
  const light = block('[data-theme="light"]');

  const missingFromLight = [...dark].filter((token) => !light.has(token));
  const missingFromDark = [...light].filter((token) => !dark.has(token));
  expect(missingFromLight, "defined for dark but not light").toEqual([]);
  expect(missingFromDark, "defined for light but not dark").toEqual([]);
  expect(dark.size).toBeGreaterThan(15);
});

it("every token the shell uses actually exists", () => {
  // A typo in `var(--surface-panl-default)` is silent: the property just does not apply.
  const tokens = TOKEN_FILES.map(path => readFileSync(path, "utf8")).join("\n");
  const defined = new Set([...tokens.matchAll(/(--[a-z0-9-]+):/g)].map((m) => m[1]));

  for (const path of [...styleSheets(), ...synthSources()]) {
    const used = [...readFileSync(path, "utf8").matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]);
    // Set by a rule or by a component rather than defined in the palette - the
    // indirection that lets one class carry any status, or any zone's colour. They are
    // not typos, and they are listed here so a real one still fails.
    const supplied = new Set<string | undefined>([
      "--status-color",
      "--seat-mark",
      "--workspace-depth",
      "--synth-signal",
    ]);
    const unknown = [...new Set(used)].filter(
      (token) => !defined.has(token) && !supplied.has(token),
    );
    expect(unknown, `${path} uses tokens that are not defined`).toEqual([]);
  }
});
