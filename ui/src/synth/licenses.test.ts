import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

it("ships the locked editor-mode and WebGL licences without altering their notices", () => {
  const root = join(import.meta.dirname, "../..");
  const notice = readFileSync(join(root, "public/licenses/synth-extensions.txt"), "utf8");
  expect(readFileSync(join(root, "dist/licenses/synth-extensions.txt"), "utf8")).toBe(notice);
  for (const name of ["@codemirror/lang-php", "@codemirror/lang-yaml", "@codemirror/legacy-modes", "@lezer/php", "@lezer/yaml", "@xterm/addon-webgl"]) {
    const directory = join(root, "node_modules", name);
    const manifest = JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
    expect(manifest.license).toBe("MIT");
    expect(notice).toContain(`=== ${name}@${manifest.version} ===`);
    expect(notice).toContain(readFileSync(join(directory, "LICENSE"), "utf8"));
  }
});
