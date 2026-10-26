import { observation, recent, type Series } from "./graph";

/** A display trail, not a sampler or a time axis. Only reported values enter it.
 * The final horizontal segment holds the last reported reading, never a new receipt. */
export class FlowTrace {
  private source: (number | null)[] = [];
  private points: { x: number; value: number | null }[] = [];
  phase = 0;
  private lastReported: number | null = null;
  reset(values: Series) {
    this.source = recent(values, 120);
    this.lastReported = this.source.filter(value => value !== null).at(-1) ?? null;
    this.points = this.source.map((value, index) => ({ value, x: this.source.length <= 1 ? 0 : index * 300 / (this.source.length - 1) }));
    this.phase = 0;
  }
  update(values: Series) {
    const next = recent(values, 120);
    if (next.length === this.source.length && next.every((value, i) => value === this.source[i])) return;
    if (!this.source.length || !next.length) { this.reset(next); return; }
    let overlap = Math.min(this.source.length, next.length);
    while (overlap && !next.slice(0, overlap).every((value, i) => value === this.source[this.source.length - overlap + i])) overlap--;
    // A live bucket may revise its tail rather than append. Append only the new
    // reading, not the old window again. Lack of value overlap is not missing
    // telemetry. Preserve explicit unknowns in the changed portion; Scope resets
    // this display when its owning node changes.
    let prefix = 0;
    while (prefix < Math.min(this.source.length, next.length) && next[prefix] === this.source[prefix]) prefix++;
    const changed = next.slice(prefix);
    const added = overlap ? next.slice(overlap) : changed.includes(null) ? [null, next.at(-1) ?? null] : [next.at(-1) ?? null];
    const first = 300 - Math.max(0, added.length - 1) * 4;
    const shift = Math.max(0, (this.points.at(-1)?.x ?? 0) - first + 4);
    for (const point of this.points) point.x -= shift;
    this.points.push(...added.map((value, i) => ({ value, x: first + i * 4 })));
    this.points = this.points.slice(-120);this.source = next;
    this.lastReported = next.filter(value => value !== null).at(-1) ?? this.lastReported;
  }
  advance(milliseconds: number) {
    const distance = Math.max(0, Math.min(milliseconds, 1000)) * .012;
    this.phase = (this.phase + distance) % 16;
    for (const point of this.points) point.x -= distance;
    // Keep one predecessor for clipping and the last held observation, not old history.
    while (this.points.length > 1 && this.points[1]!.x < 0) this.points.shift();
    if (this.points.length === 1) this.points[0]!.x = Math.max(-1, this.points[0]!.x);
  }
  samples() { return this.points.map(point => point.value); }
  /** A dashed, explicitly stale hold, never a reported observation or a bridge
   * between measured segments. An owner reset discards it even with no new data. */
  holdPath(maximum: number) {
    if (this.points.at(-1)?.value !== null || this.lastReported === null) return "";
    let x = 0;
    for (let i = this.points.length - 1; i >= 0; i--) {
      if (this.points[i]!.value !== null) { x = this.points[i]!.x; break; }
    }
    const y = 78 - Math.min(1, this.lastReported / (observation(maximum) || 1)) * 76;
    return `M${Math.max(0, x).toFixed(2)} ${y.toFixed(2)}L300.00 ${y.toFixed(2)}`;
  }
  path(maximum: number) {
    const max = observation(maximum) || 1;
    let path = "", connected = false;
    for (const point of this.points) {
      if (point.value === null) { connected = false; continue; }
      const y = 78 - Math.min(1, point.value / max) * 76;
      path += `${connected ? "L" : "M"}${point.x.toFixed(2)} ${y.toFixed(2)}`;connected = true;
    }
    const last = this.points.at(-1);
    if (last && last.value !== null) path += `L300.00 ${(78 - Math.min(1, last.value / max) * 76).toFixed(2)}`;
    return path;
  }
}
