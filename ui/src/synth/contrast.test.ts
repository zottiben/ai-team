import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

const tokens = readFileSync(join(import.meta.dirname, "tokens.css"), "utf8");
const styles = readFileSync(join(import.meta.dirname, "primitives.css"), "utf8");
const values = new Map([...tokens.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(m => [m[1], m[2]]));
function resolve(name: string): string {
  const value = values.get(name);
  if (!value) throw new Error(`Missing colour token ${name}`);
  return value.replace(/var\((--[\w-]+)\)/g, (_match, token: string) => resolve(token));
}
function colour(value: string): number[] {
  if (value.startsWith("#")) return [1, 3, 5].map(i => parseInt(value.slice(i, i + 2), 16));
  return (value.match(/[\d.]+/g) ?? []).map(Number);
}
function luminance(rgb: number[]) {
  return rgb.slice(0, 3).reduce((sum, value, i) => {
    const c = value / 255;
    return sum + (c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4) * [0.2126, 0.7152, 0.0722][i]!;
  }, 0);
}

it("small secondary text on the magenta plate meets 4.5:1 at both gradient stops", () => {
  // Browser inspection exposed 2.47:1 with the usual 62% Pearl. Use the actual
  // selector/token values so a future palette adjustment cannot silently regress it.
  const override = styles.match(/\.synth-plate\[data-tone="magenta"\] \.synth-muted\s*\{\s*color:\s*var\((--[\w-]+)\)/)?.[1];
  const foreground = colour(resolve(override ?? "--synth-text-muted"));
  const backgrounds = resolve("--plate-magenta").match(/#[a-f\d]{6}/gi) ?? [];
  expect(backgrounds).toHaveLength(2);
  for (const stop of backgrounds) {
    const background = colour(stop);
    const alpha = foreground[3] ?? 1;
    const blended = foreground.slice(0, 3).map((c, i) => c * alpha + background[i]! * (1 - alpha));
    const a = luminance(blended), b = luminance(background);
    expect((Math.max(a, b) + .05) / (Math.min(a, b) + .05)).toBeGreaterThanOrEqual(4.5);
  }
});
