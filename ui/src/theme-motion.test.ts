import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

it("reduced motion disables decorative pseudo-elements as well as ordinary elements", () => {
  const css = readFileSync(join(import.meta.dirname, "styles.css"), "utf8");
  const blocks = [...css.matchAll(/@media\s*\(prefers-reduced-motion:\s*reduce\)\s*\{\s*([^{}]+)\{([^}]+)\}/g)];
  const universal = blocks.find(block => block[1]?.split(",").map(selector => selector.trim()).includes("*"));
  expect(universal?.[1]?.split(",").map(selector => selector.trim())).toEqual(expect.arrayContaining(["*", "*::before", "*::after"]));
  expect(universal?.[2]).toMatch(/animation:\s*none\s*!important/);
  expect(universal?.[2]).toMatch(/transition:\s*none\s*!important/);
});
