import type { RepoMap } from "../../api";
import type { ChatSeat } from "../../chat-overview-api";
import type { DirectoryActivity } from "../primitives/Wavetable";
import { count } from "./telemetry";

export type RepositoryPoint = { path: string; directory: string; hits: number; owner: string | null };
/** X is lexicographic file order, never a random landscape. Only recorded touches count.
 * Paths absent from the bounded map stay counted as omitted, not silently assigned. */
export function repositoryActivity(map: RepoMap, seats: readonly Pick<ChatSeat, "touches">[]) {
  const touches = new Map<string, number>();
  for (const seat of seats) for (const touch of seat.touches) {
    const reads = count(touch.reads), writes = count(touch.writes);
    const total = reads === null || writes === null ? NaN : (touches.get(touch.path) ?? 0) + reads + writes;
    touches.set(touch.path, Number.isSafeInteger(total) ? total : NaN);
  }
  const files = map.nodes.filter(node => !node.dir).sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  const known = new Set(files.map(file => file.path));
  const omitted = [...touches.keys()].filter(path => !known.has(path)).length;
  const points: RepositoryPoint[] = files.map(file => ({ path: file.path, owner: file.owner,
    directory: file.path.includes("/") ? file.path.slice(0, file.path.indexOf("/")) : "(root)", hits: touches.get(file.path) ?? 0 }));
  const groups = new Map<string, RepositoryPoint[]>();
  for (const point of points) { const files = groups.get(point.directory) ?? []; files.push(point); groups.set(point.directory, files); }
  for (const node of map.nodes) if (node.dir && node.depth <= 1) {
    const name = node.path === "" || node.path === "." ? "(root)" : node.path.replace(/\/$/, "");
    if (!groups.has(name)) groups.set(name, []);
  }
  const directories: DirectoryActivity[] = [...groups].map(([name, files]) => ({ name,
    hits: files.map((file, index) => ({ position: files.length === 1 ? 0.5 : index / (files.length - 1), hits: file.hits })) }));
  return { points, directories, omitted };
}
