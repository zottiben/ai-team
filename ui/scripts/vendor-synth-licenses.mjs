// Run after changing these dependencies: node scripts/vendor-synth-licenses.mjs
// Copy complete notices from the locked installation, normalizing CRLF to LF.
// No network or build hook; copyright and licence wording stay unchanged.
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = join(import.meta.dirname, "..");
const packages = ["@codemirror/lang-php", "@codemirror/lang-yaml", "@codemirror/legacy-modes", "@lezer/php", "@lezer/yaml", "@xterm/addon-webgl",
  "@dnd-kit/dom", "@dnd-kit/abstract", "@dnd-kit/state", "@dnd-kit/geometry", "@dnd-kit/collision", "@preact/signals-core", "tslib"];
const notices = [];
for (const name of packages) {
  const directory = join(root, "node_modules", name);
  const manifest = JSON.parse(await readFile(join(directory, "package.json"), "utf8"));
  if (manifest.license !== (name === "tslib" ? "0BSD" : "MIT")) throw new Error(`${name}: licence changed; review before distributing`);
  const files = name === "tslib" ? ["CopyrightNotice.txt", "LICENSE.txt"] : ["LICENSE"];
  const content = await Promise.all(files.map(file => readFile(join(directory, file), "utf8")));
  notices.push(`=== ${name}@${manifest.version} ===\n\n${content.map(notice => notice.replaceAll("\r\n", "\n")).join("\n\n")}`);
}
await mkdir(join(root, "public/licenses"), { recursive: true });
await writeFile(join(root, "public/licenses/synth-extensions.txt"), notices.join("\n\n"));
