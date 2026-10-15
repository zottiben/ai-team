import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

it("ships complete locked Synth dependency notices with consistent LF endings", () => {
  const root = join(import.meta.dirname, "../..");
  const notice = readFileSync(join(root, "public/licenses/synth-extensions.txt"), "utf8");
  expect(notice).not.toContain("\r");
  expect(readFileSync(join(root, "dist/licenses/synth-extensions.txt"), "utf8")).toBe(notice);
  for (const name of ["@codemirror/lang-php", "@codemirror/lang-yaml", "@codemirror/legacy-modes", "@lezer/php", "@lezer/yaml", "@xterm/addon-webgl",
    "@dnd-kit/dom", "@dnd-kit/abstract", "@dnd-kit/state", "@dnd-kit/geometry", "@dnd-kit/collision", "@preact/signals-core", "tslib"]) {
    const directory = join(root, "node_modules", name);
    const manifest = JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
    expect(manifest.license).toBe(name === "tslib" ? "0BSD" : "MIT");
    expect(notice).toContain(`=== ${name}@${manifest.version} ===`);
    for (const file of name === "tslib" ? ["CopyrightNotice.txt", "LICENSE.txt"] : ["LICENSE"]) expect(notice).toContain(readFileSync(join(directory, file), "utf8").replaceAll("\r\n", "\n"));
  }
});
