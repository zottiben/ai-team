import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ceiling, describeSeries, plot, SampleBuffer } from "./graph";
import { Envelope } from "./Envelope";
import { Scope } from "./Scope";
import { Wavetable, directoryRidges, ridgePath } from "./Wavetable";
import { StepPads } from "./StepPads";
import { CommandPad } from "./CommandPad";
import { createReadoutClock } from "../motion/readouts";

afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("keeps a bounded chronological sample ring without fabricating missing history", () => {
  const buffer = new SampleBuffer(3);
  expect(buffer.snapshot()).toEqual([]);
  buffer.push(1); buffer.push(null); buffer.push(3); buffer.push(4);
  expect(buffer.snapshot()).toEqual([null, 3, 4]);
  buffer.push(Infinity); buffer.push(-1);
  expect(buffer.snapshot()).toEqual([4, null, null]);
  expect(() => new SampleBuffer(0)).toThrow(); expect(() => new SampleBuffer(601)).toThrow();
});

it("uses p95 rate and max context scales, clips spikes, and splits lines/fills at gaps", () => {
  expect(ceiling([...Array.from({ length: 19 }, () => 10), 10_000], "rate")).toBe(12);
  expect(ceiling([10, 20], "context")).toBeCloseTo(21.4);
  expect(ceiling([0, null], "rate")).toBe(1);
  const paths = plot([0, 10, null, 100, 0], 10);
  expect(paths.line.match(/M/g)).toHaveLength(2); expect(paths.fill.match(/Z/g)).toHaveLength(2);
  expect(paths.line).not.toMatch(/NaN|Infinity/);
  expect(describeSeries([10, null], "tok/s")).toBe("tok/s unavailable");
  expect(describeSeries([0], "tok/s")).toBe("0 tok/s");
});

it("draws isolated observations on either side of a gap as round-cap points", () => {
  const paths = plot([1, null, 3], 4);
  expect(paths.line).toBe("M0.00 59.00L0.00 59.00M300.00 21.00L300.00 21.00");
});

it("does not invent compaction markers from a fall in retained context", () => {
  const view = render(<Envelope label="Context" retained={[300, 900, 100, null, 250]} />);
  expect(screen.getByText("250 retained tokens")).toBeDefined();
  expect(view.container.querySelectorAll(".synth-graph__compaction")).toHaveLength(0);
  view.rerender(<Envelope label="Context" retained={[300, 900, 100, null, 250]} compactions={[2, 2, -1, 100]} />);
  expect(view.container.querySelectorAll(".synth-graph__compaction")).toHaveLength(1);
  expect(screen.getByText("Recorded compaction at turn 3")).toBeDefined();
  expect(view.container.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
});

it("updates reduced-motion data and text at 1Hz without scheduling animation frames", () => {
  vi.useFakeTimers();
  vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
  const raf = vi.spyOn(window, "requestAnimationFrame");
  let data = [0, 10];
  const read = vi.fn(() => data);
  const view = render(<Scope label="Rate" unit="tok/s" samples={data} read={read} active />);
  data = [10, 20]; act(() => { vi.advanceTimersByTime(500); });
  expect(screen.getByText("10 tok/s")).toBeDefined();
  act(() => { vi.advanceTimersByTime(500); }); expect(screen.getByText("20 tok/s")).toBeDefined();
  expect(raf).not.toHaveBeenCalled();
  view.unmount(); expect(vi.getTimerCount()).toBe(0);
});

it("batches readout callbacks in one timer and tears down the last subscription", () => {
  vi.useFakeTimers(); const subscribe = createReadoutClock(), a = vi.fn(), b = vi.fn();
  const offA = subscribe(a), offB = subscribe(b);
  expect(vi.getTimerCount()).toBe(1);
  act(() => { vi.advanceTimersByTime(1000); });
  expect(a).toHaveBeenCalledTimes(1); expect(b).toHaveBeenCalledTimes(1);
  offA(); offB(); expect(vi.getTimerCount()).toBe(0);
});

it("groups excess directories without losing their names and leaves empty ridges flat", () => {
  const dirs = Array.from({ length: 20 }, (_, i) => ({ name: `dir${i}`, hits: [{ position: .5, hits: i }] }));
  const ridges = directoryRidges(dirs);
  expect(ridges).toHaveLength(14); expect(ridges[13]?.names).toHaveLength(7);
  expect(ridges.flatMap(r => r.names)).toEqual(dirs.map(d => d.name));
  expect(ridgePath([], 50)).not.toMatch(/NaN|Infinity/);
  expect(ridgePath([], 50).match(/50\.00/g)).toHaveLength(101);
  expect(ridgePath([{ position: .5, hits: 20 }], 50)).not.toBe(ridgePath([], 50));
});

it("debounces repository updates but clears an unavailable source immediately", () => {
  vi.useFakeTimers(); const view = render(<Wavetable directories={[]} />);
  view.rerender(<Wavetable directories={[{ name: "src", hits: [{ position: .5, hits: 8 }] }]} />);
  act(() => { vi.advanceTimersByTime(499); }); expect(screen.getByText("No directories")).toBeDefined();
  act(() => { vi.advanceTimersByTime(1); }); expect(screen.getByText("8 hits")).toBeDefined();
  view.rerender(<Wavetable directories={null} />); expect(screen.getByText("Unavailable")).toBeDefined();
  view.unmount(); expect(vi.getTimerCount()).toBe(0);
});

it("pads preserve exact step targets and expose unknown durations and unmapped kinds", () => {
  const select = vi.fn();
  render(<StepPads label="Steps" lanes={[{ id: "edit", label: "Edit", signal: "edit" }]} onSelect={select}
    steps={[{ id: "exact-7", lane: "edit", label: "settings.ts", current: true }, { id: "exact-8", lane: "other", label: "new tool" }]} />);
  const pad = screen.getByRole("button", { name: "Edit · settings.ts · duration unavailable" });
  fireEvent.click(pad); expect(select).toHaveBeenCalledExactlyOnceWith("exact-7");
  expect(pad.getAttribute("aria-current")).toBe("step");
  expect(screen.getByText("Unmapped steps: new tool")).toBeDefined();
});

it("expires a step arrival so scrolling back cannot replay an old pop", () => {
  vi.useFakeTimers();
  const view = render(<StepPads label="Steps" lanes={[{ id: "read", label: "Read", signal: "read" }]} arriving="one"
    steps={[{ id: "one", lane: "read", label: "First" }]} />);
  expect(view.container.querySelector('[data-arriving="true"]')).not.toBeNull();
  act(() => { vi.advanceTimersByTime(160); });
  expect(view.container.querySelector('[data-arriving="true"]')).toBeNull();
});

it("command pads only call the host on explicit press, retain exit text and expire result LEDs", () => {
  vi.useFakeTimers(); vi.setSystemTime(10_000); const run = vi.fn();
  const view = render(<CommandPad label="Tests" command="npm test" onActivate={run} />);
  expect(run).not.toHaveBeenCalled(); fireEvent.click(screen.getByRole("button")); expect(run).toHaveBeenCalledTimes(1);
  view.rerender(<CommandPad label="Tests" command="npm test" onActivate={run} running />);
  fireEvent.click(screen.getByRole("button")); expect(run).toHaveBeenCalledTimes(1); expect(screen.getByText("Running")).toBeDefined();
  view.rerender(<CommandPad label="Tests" command="npm test" onActivate={run} result={{ id: "receipt-1", exitCode: 1, finishedAt: 10_000 }} />);
  expect(view.container.querySelector(".synth-led")?.getAttribute("data-state")).toBe("on");
  act(() => { vi.advanceTimersByTime(2000); });
  expect(view.container.querySelector(".synth-led")?.getAttribute("data-state")).toBe("off"); expect(screen.getByText("Exit 1")).toBeDefined();
  view.rerender(<CommandPad label="Tests" command="npm test" onActivate={run} blockedReason="Read-only mirror" />);
  fireEvent.click(screen.getByRole("button")); expect(run).toHaveBeenCalledTimes(1); expect(screen.getByText("Read-only mirror")).toBeDefined();
});
