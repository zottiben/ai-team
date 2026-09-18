import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
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

it("the window loads without the editor or the terminal", () => {
  // Every other view works without either, so neither may be in the eager chunk.
  const eager = readFileSync(join(DIST, "app.js"), "utf8");
  expect(eager).not.toContain("@codemirror/state");
  expect(eager).not.toContain("xterm");
});

it("the eager bundle stays under its budget", () => {
  // 400k of headroom over roughly 250k today. Crossing this means something large became
  // eager, and the fix is a lazy import rather than a bigger number.
  const budget = 400 * 1024;
  const actual = sizeOf("app.js");
  expect(actual, `app.js is ${Math.round(actual / 1024)}KB`).toBeLessThan(budget);
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
