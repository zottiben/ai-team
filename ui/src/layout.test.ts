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
