export interface Point { x: number; y: number }
export const spring = (seconds: number) => seconds >= 1.2 ? 1 : 1 - Math.exp(-5 * Math.max(0, seconds)) * Math.cos(12 * Math.max(0, seconds));

export function cablePath(a: Point, b: Point, factor = 1): string {
  const dx = Math.abs(b.x - a.x), dy = Math.abs(b.y - a.y);
  const low = Math.max(a.y, b.y) + Math.min(118, 34 + 0.11 * dx + 0.05 * dy) * factor;
  return `M${a.x} ${a.y} C${a.x + (b.x - a.x) * 0.12} ${low} ${b.x - (b.x - a.x) * 0.12} ${low} ${b.x} ${b.y}`;
}
