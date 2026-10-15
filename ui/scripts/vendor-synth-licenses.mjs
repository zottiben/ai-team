// Run after changing these dependencies: node scripts/vendor-synth-licenses.mjs
// Copy unmodified MIT notices from the locked installation; no network or build hook.
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = join(import.meta.dirname, "..");
const packages = ["@codemirror/lang-php", "@codemirror/lang-yaml", "@codemirror/legacy-modes", "@lezer/php", "@lezer/yaml", "@xterm/addon-webgl"];
const notices = [];
for (const name of packages) {
  const directory = join(root, "node_modules", name);
  const manifest = JSON.parse(await readFile(join(directory, "package.json"), "utf8"));
  if (manifest.license !== "MIT") throw new Error(`${name}: licence changed; review before distributing`);
  notices.push(`=== ${name}@${manifest.version} ===\n\n${await readFile(join(directory, "LICENSE"), "utf8")}`);
}
await mkdir(join(root, "public/licenses"), { recursive: true });
await writeFile(join(root, "public/licenses/synth-extensions.txt"), notices.join("\n\n"));
