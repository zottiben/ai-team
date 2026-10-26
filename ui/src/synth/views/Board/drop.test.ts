import { expect, it } from "vitest";
import type { PlanSlice } from "../../../plan-api";
import { currentDrop, statusPosition, type StatusDrop } from "./drop";
const slices: PlanSlice[] = ["S8", "S1", "S9"].map((key, index) => ({ id: index, key, title: key, status: index === 1 ? "draft" : "ready", scope_md: "", demo_md: null, blocked_reason: null, rev: 1 }));
const drop: StatusDrop = { key: "S1", from: "draft", to: "ready", revision: 4 };
it("admits only an exact current editable source and a different status", () => {
  const view = { slices, revision: 4, disabled: false };
  expect(currentDrop(view, drop)).toBe(true);
  expect(currentDrop({ ...view, disabled: true }, drop)).toBe(false);
  expect(currentDrop({ ...view, revision: 5 }, drop)).toBe(false);
  expect(currentDrop(view, { ...drop, revision: NaN })).toBe(false);
  expect(currentDrop(view, { ...drop, from: "active" })).toBe(false);
  expect(currentDrop(view, { ...drop, to: "draft" })).toBe(false);
  expect(currentDrop(view, { ...drop, key: "missing" })).toBe(false);
  expect(currentDrop({ ...view, slices: [...slices, slices[1]!] }, drop)).toBe(false);
});
it("derives a position without sorting or creating a rank mutation", () => {
  expect(statusPosition(slices, "S1", "ready")).toBe(2);
  expect(statusPosition(slices, "S1", "active")).toBe(1);
  expect(statusPosition(slices, "missing", "ready")).toBeNull();
  expect(slices.map(slice => slice.key)).toEqual(["S8", "S1", "S9"]);
});
