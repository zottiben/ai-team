export type Series = ArrayLike<number | null>;
export const observation = (value: number | null | undefined): number | null =>
  typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;

/** Collection belongs to the data source, never to an animation frame. */
export class SampleBuffer {
  private readonly values: Float32Array;
  private head = 0;
  private count = 0;
  constructor(readonly capacity = 120) {
    if (!Number.isInteger(capacity) || capacity < 1 || capacity > 600) throw new Error("Sample capacity must be 1…600");
    this.values = new Float32Array(capacity);
  }
  push(value: number | null): void {
    this.values[this.head] = observation(value) ?? NaN;
    this.head = (this.head + 1) % this.capacity;
    this.count = Math.min(this.count + 1, this.capacity);
  }
  snapshot(): (number | null)[] {
    return Array.from({ length: this.count }, (_, i) => observation(this.values[(this.head - this.count + i + this.capacity) % this.capacity]));
  }
}

export function recent(series: Series, limit: number): (number | null)[] {
  return Array.from({ length: Math.min(limit, series.length) }, (_, i) => observation(series[Math.max(0, series.length - limit) + i]));
}

export function ceiling(series: Series, mode: "rate" | "context"): number {
  const values = Array.from(series).map(observation).filter((v): v is number => v !== null).sort((a, b) => a - b);
  const value = mode === "rate" ? values[Math.max(0, Math.ceil(values.length * .95) - 1)] : values.at(-1);
  return Math.max(1, (value ?? 0) * (mode === "rate" ? 1.2 : 1.07));
}

/** Unknown points break both line and fill; they are not zero or interpolated history. */
export function plot(series: Series, max: number, width = 300, height = 80): { line: string; fill: string } {
  max = observation(max) || 1;
  const bottom = height - 2;
  let line = "", fill = "", segment = "", start = 0, end = 0, count = 0, lastPoint = "";
  const close = () => {
    // An SVG move alone paints nothing: preserve isolated observations with a
    // zero-length, round-capped segment instead of losing the newest point.
    if (count === 1) { segment += `L${lastPoint}`; line += `L${lastPoint}`; }
    if (segment) fill += `${segment} L${end} ${bottom} L${start} ${bottom} Z `;
    segment = ""; count = 0;
  };
  Array.from(series).forEach((raw, i) => {
    const value = observation(raw);
    if (value === null) { close(); return; }
    const x = series.length <= 1 ? width / 2 : i / (series.length - 1) * width;
    const y = bottom - Math.min(1, value / max) * (height - 4);
    lastPoint = `${x.toFixed(2)} ${y.toFixed(2)}`;
    const command = `${segment ? "L" : "M"}${lastPoint}`;
    if (!segment) start = x;
    segment += command; line += command; end = x; count++;
  });
  close();
  return { line, fill };
}

export function describeSeries(series: Series, unit: string): string {
  if (!series.length) return "No samples";
  const value = observation(series[series.length - 1]);
  if (value === null) return `${unit} unavailable`;
  return `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })} ${unit}`;
}
