import { describe, expect, it, vi } from "vitest";
import { createTicker } from "./ticker";
import { advanceVu } from "./ballistics";
import { cablePath, spring } from "./spring";

function clock() {
  let id = 0;
  const queue = new Map<number, FrameRequestCallback>();
  const request = vi.fn((fn: FrameRequestCallback) => { queue.set(++id, fn); return id; });
  const cancel = vi.fn((handle: number) => { queue.delete(handle); });
  const ticker = createTicker(request, cancel);
  return { ticker, queue, request, cancel, frame(now: number) {
    const work = [...queue.values()]; queue.clear(); work.forEach(fn => fn(now));
  } };
}

describe("shared synth clock", () => {
  it("has one loop, per-subscriber caps and no accumulated hidden dt", () => {
    const c = clock(), slow = vi.fn(), fast = vi.fn();
    const a = c.ticker.subscribe(slow, 1), b = c.ticker.subscribe(fast, 30);
    expect(c.queue.size).toBe(1);
    c.frame(100); c.frame(120); c.frame(134);
    expect(slow.mock.calls).toEqual([[100, 0]]);
    expect(fast.mock.calls).toEqual([[100, 0], [134, 34]]);
    a.visible(false); b.visible(false);
    expect(c.queue.size).toBe(0);
    b.visible(true); c.frame(10_000);
    expect(fast.mock.lastCall).toEqual([10_000, 0]);
    b.unsubscribe(); a.unsubscribe();
    expect(c.queue.size).toBe(0);
  });
  it("does not request any frames for initially invisible subscribers", () => {
    const c = clock(), callback = vi.fn();
    const sub = c.ticker.subscribe(callback, 20, false);
    expect(c.request).not.toHaveBeenCalled();
    sub.visible(true); c.frame(0);
    c.ticker.pause(true);
    expect(c.queue.size).toBe(0);
    c.ticker.pause(false); c.frame(100_000);
    expect(callback.mock.lastCall).toEqual([100_000, 0]);
    sub.unsubscribe();
  });
  it("stops finite effects and tolerates removing another subscriber in a callback", () => {
    const c = clock(), removed = vi.fn();
    const a = c.ticker.subscribe(() => { b.unsubscribe(); return false; }, 30);
    const b = c.ticker.subscribe(removed, 20);
    c.frame(0);
    expect(removed).not.toHaveBeenCalled();
    expect(c.queue.size).toBe(0);
    a.unsubscribe();
    expect(c.queue.size).toBe(0);
  });
  it.each([0, -1, Infinity, NaN, 61])("rejects invalid fps %s", fps => {
    expect(() => clock().ticker.subscribe(() => {}, fps)).toThrow("fps");
  });
});

it("uses instant VU attack, 0.8/s release and one-second peak hold then 0.6/s", () => {
  let state = advanceVu({ level: 0, peak: 0, held: 0 }, 1, 0);
  expect(state).toEqual({ level: 1, peak: 1, held: 1 });
  state = advanceVu(state, 0, .5);
  expect(state).toEqual({ level: .6, peak: 1, held: .5 });
  state = advanceVu(state, 0, .5);
  expect(state.level).toBeCloseTo(.2); expect(state.peak).toBe(1);
  state = advanceVu(state, 0, .5);
  expect(state).toEqual({ level: 0, peak: .7, held: 0 });
  expect(advanceVu(state, 0, 2)).toEqual({ level: 0, peak: 0, held: 0 });
  expect(advanceVu(state, NaN, 0).level).toBe(0);
});

it("starts the cable taut, overshoots and reaches the exact final sag", () => {
  expect(spring(0)).toBe(0);
  expect(spring(.25)).toBeGreaterThan(1);
  expect(spring(1.2)).toBe(1);
  expect(cablePath({ x: 0, y: 0 }, { x: 100, y: 0 }, 0)).toBe("M0 0 C12 0 88 0 100 0");
  expect(cablePath({ x: 0, y: 0 }, { x: 100, y: 0 })).toBe("M0 0 C12 45 88 45 100 0");
});
