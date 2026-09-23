import { act, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { activityOf } from "./activity";
import type { RepoMap } from "./api";
import { RepoGraph } from "./Map";

/** A root, two zones of files, and one file nobody claims. */
const MAP: RepoMap = {
  root: "widget",
  nodes: [
    { path: "", name: "widget", dir: true, depth: 0, weight: 5, owner: null },
    { path: "src", name: "src", dir: true, depth: 1, weight: 2, owner: "backend" },
    { path: "src/greet.sh", name: "greet.sh", dir: false, depth: 2, weight: 1, owner: "backend" },
    { path: "src/shout.sh", name: "shout.sh", dir: false, depth: 2, weight: 1, owner: "backend" },
    { path: "web", name: "web", dir: true, depth: 1, weight: 2, owner: "frontend" },
    { path: "web/index.html", name: "index.html", dir: false, depth: 2, weight: 1, owner: "frontend" },
    { path: "web/check.sh", name: "check.sh", dir: false, depth: 2, weight: 1, owner: "frontend" },
    { path: "README.md", name: "README.md", dir: false, depth: 1, weight: 1, owner: null },
  ],
  edges: [
    { from: 0, to: 1 },
    { from: 1, to: 2 },
    { from: 1, to: 3 },
    { from: 0, to: 4 },
    { from: 4, to: 5 },
    { from: 4, to: 6 },
    { from: 0, to: 7 },
  ],
  zones: [
    { role: "backend", name: "Backend", zone: "src/**", owns: 2 },
    { role: "frontend", name: "Frontend", zone: "web/**", owns: 2 },
  ],
  unowned: 1,
  files: 5,
  truncated: false,
};

const QUIET = activityOf(MAP, [], []);

/** The drawing's own units, as `[x, y, width, height]`. */
function viewBox(container: HTMLElement): number[] {
  const box = container.querySelector("svg.map")?.getAttribute("viewBox") ?? "";
  return box.split(" ").map(Number);
}

/** Every element in the document reports this width, as a panel of it would. */
function drawnAt(width: number) {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue(
    DOMRect.fromRect({ x: 0, y: 0, width, height: 200 }),
  );
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it("lays the map out at the width its panel draws it, so a label is the size the stylesheet says", () => {
  // One column of the Overview is about 360 pixels. A map solved for a 1200-unit band and
  // scaled down into it drew its 11px zone labels three pixels tall.
  drawnAt(360);

  const { container } = render(<RepoGraph map={MAP} activity={QUIET} />);

  const [x, y, width, height] = viewBox(container);
  expect([x, width]).toEqual([0, 360]);
  // Squarer than a band, or one column's picture is a strip too thin to read.
  expect(y).toBeGreaterThanOrEqual(0);
  expect(height).toBeGreaterThan(150);
  expect(height).toBeLessThanOrEqual(240);
});

it("lays it out again once the panel has settled at a new width", () => {
  vi.useFakeTimers();
  let resized: ResizeObserverCallback = () => {};
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: ResizeObserverCallback) {
        resized = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  drawnAt(360);
  const { container } = render(<RepoGraph map={MAP} activity={QUIET} />);

  const widen = (width: number) =>
    resized(
      [{ contentRect: DOMRect.fromRect({ width, height: 200 }) } as ResizeObserverEntry],
      {} as ResizeObserver,
    );
  act(() => {
    widen(600);
    widen(840);
  });
  // Mid-drag the picture it has is scaled rather than solved again at every step.
  expect(viewBox(container)[2]).toBe(360);

  act(() => {
    vi.advanceTimersByTime(1000);
  });
  expect(viewBox(container)[2]).toBe(840);
});

it("draws the whole band when there is no layout to measure it by", () => {
  // A document with no layout reports a width of nothing - which is no reason to draw
  // nothing.
  const { container } = render(<RepoGraph map={MAP} activity={QUIET} />);

  const [x, , width] = viewBox(container);
  expect([x, width]).toEqual([0, 1200]);
});

it("keeps every zone label on a plate inside the drawing, however narrow the panel", () => {
  // A zone's label sits on its cluster, which is where the cluster's hub is: without a
  // plate the hub shows through the gaps in `frontend · 3`. And a cluster at the edge of a
  // narrow panel would have its label cut in half.
  drawnAt(140);

  const { container } = render(<RepoGraph map={MAP} activity={QUIET} />);

  const [, , width] = viewBox(container);
  const labels = [...container.querySelectorAll(".map__labels > g")];
  expect(labels.map((label) => label.querySelector("text")?.textContent)).toEqual([
    "backend · 2",
    "frontend · 2",
  ]);
  for (const label of labels) {
    const plate = label.querySelector("rect.map__label-plate");
    expect(plate).not.toBeNull();
    const x = Number(plate?.getAttribute("x"));
    expect(x).toBeGreaterThanOrEqual(0);
    expect(x + Number(plate?.getAttribute("width"))).toBeLessThanOrEqual(width ?? 0);
  }
});

it("puts a zone's label beside its paths rather than over them, when there is room", () => {
  // A cluster's middle is where its hub is. A plate there hides the hub - the point the
  // zone's lines meet at - behind the name of the zone.
  drawnAt(360);

  const { container } = render(<RepoGraph map={MAP} activity={QUIET} />);

  const points = [...container.querySelectorAll("circle")].map((circle) => ({
    x: Number(circle.getAttribute("cx")),
    y: Number(circle.getAttribute("cy")),
    r: Number(circle.getAttribute("r")),
  }));
  const [, top, , tall] = viewBox(container);
  for (const plate of container.querySelectorAll("rect.map__label-plate")) {
    const x = Number(plate.getAttribute("x"));
    const y = Number(plate.getAttribute("y"));
    const w = Number(plate.getAttribute("width"));
    const h = Number(plate.getAttribute("height"));
    const hidden = points.filter(
      (p) => p.x + p.r > x && p.x - p.r < x + w && p.y + p.r > y && p.y - p.r < y + h,
    );
    expect(hidden).toEqual([]);
    expect(y).toBeGreaterThanOrEqual(top ?? 0);
    expect(y + h).toBeLessThanOrEqual((top ?? 0) + (tall ?? 0));
  }
});
