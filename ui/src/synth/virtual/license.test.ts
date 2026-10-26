import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

it("ships both virtualizer packages' unmodified MIT notice with the embedded assets", () => {
  const root = join(import.meta.dirname, "../../..");
  const notice = readFileSync(join(root, "public/licenses/tanstack-virtual.txt"), "utf8");
  for (const name of ["react-virtual", "virtual-core"]) {
    expect(notice).toContain(readFileSync(join(root, `node_modules/@tanstack/${name}/LICENSE`), "utf8").trim());
  }
  expect(readFileSync(join(root, "dist/licenses/tanstack-virtual.txt"), "utf8")).toBe(notice);
});
