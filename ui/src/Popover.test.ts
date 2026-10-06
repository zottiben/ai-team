import { expect, it } from "vitest";
import { popoverPosition } from "./Popover";

it("anchors beside the trigger, clamps near viewport edges, and adapts to a narrow window", () => {
  expect(popoverPosition({ top: 24, right: 262 }, 448, 400, 1440, 900)).toEqual({ left: 270, top: 24, width: 448, maxHeight: 884 });
  const edge = popoverPosition({ top: 740, right: 1020 }, 448, 400, 1024, 768);
  expect(edge.left + edge.width).toBeLessThanOrEqual(1016);
  expect(edge.top + 400).toBeLessThanOrEqual(760);
  const narrow = popoverPosition({ top: 24, right: 42 }, 448, 500, 320, 480);
  expect(narrow).toEqual({ left: 8, top: 8, width: 304, maxHeight: 464 });
});
