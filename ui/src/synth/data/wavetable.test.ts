import { expect, it } from "vitest";
import type { RepoMap } from "../../api";
import type { SeatTouch } from "../../chat-overview-api";
import { repositoryActivity } from "./wavetable";

const map = (): RepoMap => ({ root: "fixture", files: 3, unowned: 1, truncated: true, zones: [], edges: [], nodes: [
  { path: "src/z.ts", name: "z.ts", dir: false, depth: 2, weight: 1, owner: "maker" },
  { path: "empty", name: "empty", dir: true, depth: 1, weight: 0, owner: null },
  { path: "src/a.ts", name: "a.ts", dir: false, depth: 2, weight: 1, owner: "maker" },
  { path: "README.md", name: "README.md", dir: false, depth: 1, weight: 1, owner: null },
] });
const touch = (path: string, reads: number, writes: number): SeatTouch => ({ path, reads, writes, tool: "read", at: "recorded" });

it("uses stable actual file order and recorded touch counts without mutating the map", () => {
  const source = map(), before = structuredClone(source);
  const data = repositoryActivity(source, [{ touches: [touch("src/a.ts", 2, 1), touch("not-in-map.ts", 5, 0)] }, { touches: [touch("src/a.ts", 0, 3)] }]);
  expect(source).toEqual(before);
  expect(data.points.map(point => [point.path, point.hits, point.owner])).toEqual([["README.md", 0, null], ["src/a.ts", 6, "maker"], ["src/z.ts", 0, "maker"]]);
  expect(data.directories.find(dir => dir.name === "src")?.hits).toEqual([{ position: 0, hits: 6 }, { position: 1, hits: 0 }]);
  expect(data.directories.find(dir => dir.name === "empty")?.hits).toEqual([]);
  expect(data.omitted).toBe(1);
});

it("groups nested files into their actual top-level directory", () => {
  const source = map(); source.files += 1;
  source.nodes.push({ path: "src/nested", name: "nested", dir: true, depth: 2, weight: 1, owner: "maker" }, { path: "src/nested/b.ts", name: "b.ts", dir: false, depth: 3, weight: 1, owner: "maker" });
  const data = repositoryActivity(source, [{ touches: [touch("src/nested/b.ts", 4, 0)] }]);
  expect(data.directories.find(dir => dir.name === "src")?.hits).toEqual([{ position: 0, hits: 0 }, { position: .5, hits: 4 }, { position: 1, hits: 0 }]);
  expect(data.directories.some(dir => dir.name === "src/nested")).toBe(false);
});

it("keeps invalid or overflowing counts missing rather than synthesizing activity", () => {
  const data = repositoryActivity(map(), [{ touches: [touch("src/a.ts", -1, 0), touch("src/z.ts", Number.MAX_SAFE_INTEGER, 1)] }]);
  expect(data.points.filter(point => point.path.startsWith("src/")).every(point => Number.isNaN(point.hits))).toBe(true);
  expect(repositoryActivity(map(), []).points.every(point => point.hits === 0)).toBe(true);
});
