import type { Point } from "../motion/spring";

/** Convert viewport coordinates into the unscaled SVG/host coordinate space. */
export function patchPoint(point: Point, rect: Pick<DOMRect, "left" | "top" | "width" | "height">, width: number, height: number): Point {
  const sx = width > 0 && rect.width > 0 ? rect.width / width : 1;
  const sy = height > 0 && rect.height > 0 ? rect.height / height : 1;
  return { x: (point.x - rect.left) / sx, y: (point.y - rect.top) / sy };
}
