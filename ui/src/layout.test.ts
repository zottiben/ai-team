import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

// There is no browser on the machine this is built on (rule 11), and jsdom does no layout,
// so these read the rules themselves, for layouts that broke where nobody could look.
const css = readFileSync(join(import.meta.dirname, "styles.css"), "utf8");

function rule(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return css.match(new RegExp(`\\n${escaped} \\{([^}]*)\\}`))?.[1] ?? "";
}

// The review is a flex column held to the window's height, and a file card that may
// shrink - with its overflow hidden - was squeezed to a sliver: every file of a PR but
// the largest was a few pixels of border.
it("a file in a review keeps its height, however many files there are", () => {
  expect(rule(".diff")).toMatch(/flex:\s*none/);
});

// A Today row is also a `.card`, which is a column. Without a direction of its own the rank
// sat centred above the item, the arrow centred below it, and the item's text was squeezed
// to the middle of the card.
it("a Today row lays its rank, item and arrow out in a row", () => {
  expect(rule(".today-row")).toMatch(/flex-direction:\s*row/);
});

// Thirty-odd buttons are disabled while their action runs or when it cannot apply, and
// `.button` had no disabled state: they looked pressable and lit up on hover all the same.
it("a disabled button looks it, and does not answer the pointer", () => {
  expect(rule(".button:disabled")).toMatch(/opacity:/);
  expect(rule(".button:disabled")).toMatch(/cursor:\s*not-allowed/);
  expect(rule(".button:hover")).toBe("");
  expect(rule(".button:hover:not(:disabled)")).toMatch(/background:/);
  expect(rule(".button--primary:hover")).toBe("");
  expect(rule(".button--primary:hover:not(:disabled)")).toMatch(/background:/);
});

// A row of buttons choosing one option - Settings' appearance - marks the chosen one with
// aria-current, which `.button` never drew, so nothing said which theme was in force.
it("the chosen button of a set is drawn as chosen", () => {
  expect(rule('.button[aria-current="true"]')).toMatch(/background:/);
});

// Selects were styled container by container, so one in a new place - the activity run
// picker, the layout menu's widths - rendered as the browser's own grey box in Arial.
it("a select looks like the window's other controls wherever it is", () => {
  const base = rule("select");
  expect(base).toMatch(/font:\s*inherit/);
  expect(base).toMatch(/background:\s*var\(--surface-canvas-default\)/);
  expect(base).toMatch(/border:\s*1px solid var\(--border-subtle-default\)/);
  expect(base).toMatch(/border-radius:\s*var\(--radius-sm\)/);
});

// A board card's foot - who holds it, where, which seats, how big - was one row whose
// chips could each shrink to nothing, so in a board column every one was cut to an
// ellipsis: "me…  …/1/ai…  backend + …  ~…". It wraps instead, and a chip gives up
// width only when it alone is wider than the card.
it("a board card's facts wrap rather than all being cut short", () => {
  expect(rule(".board-card__foot")).toMatch(/flex-wrap:\s*wrap/);
  expect(rule(".board-chip")).toMatch(/flex:\s*none/);
  expect(rule(".board-chip")).toMatch(/max-width:\s*100%/);
});

// The editor tree's caret slot is as wide for a file, which has no caret, as for a folder,
// so every name in a level starts at the same place.
it("a tree row's caret slot has one width, caret or not", () => {
  expect(rule(".tree__caret")).toMatch(/flex:\s*none/);
  expect(rule(".tree__caret")).toMatch(/\bwidth:/);
});

// What a seat may do is coloured, but still: only work that is happening pulses.
it("a seat's access has a colour of its own and no pulse", () => {
  expect(rule('.status[data-access="writes"]')).toMatch(/--status-color:/);
  expect(css).not.toMatch(/data-access[^{]*::before[^{]*\{[^}]*animation/);
});
