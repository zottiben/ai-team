import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

// There is no browser on the machine this is built on (rule 11), and jsdom does no layout,
// so this reads the rule itself. The review is a flex column held to the window's height,
// and a file card that may shrink - with its overflow hidden - was squeezed to a sliver:
// every file of a PR but the largest was a few pixels of border.
it("a file in a review keeps its height, however many files there are", () => {
  const css = readFileSync(join(import.meta.dirname, "styles.css"), "utf8");
  const rule = css.match(/\n\.diff \{([^}]*)\}/)?.[1] ?? "";
  expect(rule).toMatch(/flex:\s*none/);
});
