import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

const publicDir = join(import.meta.dirname, "../../public/fonts/synth");
const distDir = join(import.meta.dirname, "../../dist/fonts/synth");
const css = readFileSync(join(import.meta.dirname, "fonts.css"), "utf8");
const manifest: { files: { file: string; source: string; sha256: string }[] } = JSON.parse(readFileSync(join(publicDir, "sources.json"), "utf8"));

it("ships only local font faces with swap, source hashes and freely redistributable licences", () => {
  expect(css).not.toMatch(/url\(["']?https?:/);
  const urls = [...css.matchAll(/url\(([^)]+)\)/g)].map(m => m[1]!);
  expect(urls).toHaveLength(14);
  expect(css.match(/font-display: swap;/g)).toHaveLength(14);
  expect(css.match(/font-weight: 300 900;/g)).toHaveLength(4);
  for (const url of urls) {
    expect(url.startsWith("/fonts/synth/")).toBe(true);
    const name = url.slice("/fonts/synth/".length);
    expect(manifest.files.some(file => file.file === name)).toBe(true);
    expect(readFileSync(join(publicDir, name)).subarray(0, 4).toString()).toBe("wOF2");
  }
  for (const family of ["figtree", "ibmplexmono"]) {
    const licence = readFileSync(join(publicDir, `${family}-OFL.txt`), "utf8");
    expect(licence).toContain("SIL OPEN FONT LICENSE Version 1.1");
    expect(licence).toContain("Copyright");
    expect(licence).toContain("bundled");
    expect(licence).not.toMatch(/\r|[\t ]+$/m);
  }
  let size = 0;
  for (const file of manifest.files) {
    const bytes = readFileSync(join(publicDir, file.file));
    expect(createHash("sha256").update(bytes).digest("hex"), file.file).toBe(file.sha256);
    size += bytes.length;
  }
  expect(size).toBeLessThan(200 * 1024);
});

it("includes unmodified fonts, licences and provenance in the embedded distribution", () => {
  for (const name of readdirSync(publicDir)) {
    expect(readFileSync(join(distDir, name)), name).toEqual(readFileSync(join(publicDir, name)));
  }
});
