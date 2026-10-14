import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, normalize } from "node:path";
import ts from "typescript";
import { expect, it } from "vitest";

/**
 * What the window costs to open.
 *
 * The bundle is compiled into the binary and served from loopback, so this is not about
 * download time - it is about the work the browser does before anything is on screen, and
 * about noticing when a heavy dependency quietly becomes eager. CodeMirror and xterm are
 * each larger than the whole rest of the app; the moment one is imported at the top of a
 * shared module, the cost moves to everybody and nothing says so.
 *
 * Budgets with headroom, not high-water marks: they exist to catch a change of shape, not
 * to be nudged up every time a feature lands.
 */
const DIST = join(import.meta.dirname, "..", "dist");

function sizeOf(name: string): number {
  return statSync(join(DIST, name)).size;
}

/** Shared static imports cost startup time too; dynamic imports do not. */
function eagerChunks(): Map<string, string> {
  const found = new Map<string, string>();
  const visit = (name: string) => {
    if (found.has(name)) return;
    const source = readFileSync(join(DIST, name), "utf8");
    found.set(name, source);
    const module = ts.createSourceFile(name, source, ts.ScriptTarget.Latest, false, ts.ScriptKind.JS);
    for (const statement of module.statements) {
      if (!ts.isImportDeclaration(statement) && !ts.isExportDeclaration(statement)) continue;
      const specifier = statement.moduleSpecifier;
      if (!specifier || !ts.isStringLiteral(specifier)) continue;
      expect(specifier.text.startsWith("."), "Unexpected external startup import").toBe(true);
      const next = normalize(join(dirname(name), specifier.text));
      expect(next.startsWith(".."), "Startup import escapes the bundle").toBe(false);
      visit(next);
    }
  };
  visit("app.js");
  return found;
}
const eager = eagerChunks();

it("the window loads without the editor or the terminal", () => {
  // Every other view works without either, so neither may be in any eager chunk.
  for (const source of eager.values()) {
    expect(source).not.toContain("@codemirror/state");
    expect(source).not.toContain("xterm");
  }
});

it("the eager bundle stays under its budget", () => {
  // 400KiB for the complete static startup graph. Splitting code into shared imports
  // does not make it lazy; fix a regression with deferred loading, not a bigger number.
  const budget = 400 * 1024;
  const actual = [...eager.keys()].reduce((total, name) => total + sizeOf(name), 0);
  expect(actual, `Startup chunks (${[...eager.keys()].join(", ")}) total ${actual} bytes`).toBeLessThan(budget);
});

it("the heavy views are split out rather than merged in", () => {
  // If these ever vanish, they did not get smaller - they got folded into app.js.
  const chunks = readdirSync(DIST).filter((name) => name.endsWith(".js"));
  expect(chunks).toContain("Editor.js");
  expect(chunks).toContain("Terminal.js");
});

it("no single chunk is large enough to stall a slow machine", () => {
  // A megabyte of JavaScript is about a second of parsing on a tired laptop, and this is
  // a tool that sits open all day next to a compiler.
  const budget = 1024 * 1024;
  for (const name of readdirSync(DIST).filter((file) => file.endsWith(".js"))) {
    const actual = sizeOf(name);
    expect(actual, `${name} is ${Math.round(actual / 1024)}KB`).toBeLessThan(budget);
  }
});
