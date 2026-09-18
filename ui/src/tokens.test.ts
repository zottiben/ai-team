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
const COLOUR = /#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(|\boklch\(/;

function styleSheets(): string[] {
  return readdirSync(SRC)
    .filter((name) => name.endsWith(".css") && name !== "tokens.css")
    .map((name) => join(SRC, name));
}

it("only the token layer names a colour", () => {
  for (const path of styleSheets()) {
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
  const tokens = readFileSync(join(SRC, "tokens.css"), "utf8");
  const defined = new Set([...tokens.matchAll(/(--[a-z0-9-]+):/g)].map((m) => m[1]));

  for (const path of styleSheets()) {
    const used = [...readFileSync(path, "utf8").matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]);
    const unknown = [...new Set(used)].filter(
      // `--status-color` is set by the rules themselves, as the indirection that lets one
      // class carry any status.
      (token) => !defined.has(token) && token !== "--status-color",
    );
    expect(unknown, `${path} uses tokens that are not defined`).toEqual([]);
  }
});
