import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";
import { SYNTH_PALETTES } from "../synth-palettes";

const css = readFileSync(join(import.meta.dirname, "tokens.css"), "utf8");
const blocks = [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]+)\}/g)];
const tokens = (body: string) => new Map([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(m => [m[1]!, m[2]!]));

it("every alternative replaces all colour roles, not just its accent", () => {
  const original = tokens(blocks[0]![2]!);
  const wiring = tokens(blocks.at(-1)![2]!);
  const colours = [...original].filter(([, value]) => /#|rgba?\(|gradient\(|var\(/.test(value));
  expect(colours.length).toBeGreaterThan(150);
  for (const [name] of colours) expect(wiring.has(name), name).toBe(true);
  for (const { id } of SYNTH_PALETTES.slice(1)) {
    const matching = blocks.filter(block => block[1]!.includes(`[data-synth-palette="${id}"]`));
    expect(matching, id).toHaveLength(3); // dark, daylight, shared semantic map
    const dark = tokens(matching[0]![2]!), light = tokens(matching[1]![2]!);
    for (const key of light.keys()) expect(dark.has(key), `${id}: ${key}`).toBe(true);
    for (const name of ["--p-screen", "--p-floor", "--p-panel", "--p-text", "--p-muted", "--p-red", "--p-green", "--p-yellow", "--p-blue", "--p-purple", "--p-cyan", "--p-pink"]) {
      expect(dark.get(name), `${id} ${name}`).toBeDefined();
      expect(light.get(name), `${id} ${name}`).toBeDefined();
      expect(light.get(name)).not.toBe(dark.get(name));
    }
  }
});
