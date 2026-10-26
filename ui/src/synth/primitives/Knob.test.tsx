import { fireEvent, render, screen, act } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useState } from "react";
import { Knob, arc } from "./Knob";

const captureMethods = ["setPointerCapture", "releasePointerCapture", "hasPointerCapture"] as const;
const descriptors = captureMethods.map(key => Object.getOwnPropertyDescriptor(HTMLElement.prototype, key));
beforeEach(() => {
  vi.useFakeTimers();
  // jsdom lacks pointer capture. Model ownership instead of pretending every ID owns it.
  const captures = new WeakMap<Element, number>();
  vi.stubGlobal("PointerEvent", class extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) { super(type, init); this.pointerId = init.pointerId ?? 1; }
  });
  Object.defineProperties(HTMLElement.prototype, {
    setPointerCapture: { configurable: true, value(this: Element, id: number) { captures.set(this, id); } },
    releasePointerCapture: { configurable: true, value(this: Element) { captures.delete(this); } },
    hasPointerCapture: { configurable: true, value(this: Element, id: number) { return captures.get(this) === id; } },
  });
});
afterEach(() => {
  vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals();
  captureMethods.forEach((key, i) => {
    const descriptor = descriptors[i];
    if (descriptor) Object.defineProperty(HTMLElement.prototype, key, descriptor);
    else Reflect.deleteProperty(HTMLElement.prototype, key);
  });
});
const wait = () => act(() => { vi.advanceTimersByTime(250); });

it("debounces wheel/key settings and accumulates from the draft, not stale props", () => {
  const commit = vi.fn();
  render(<Knob label="Thinking" value={0} detents={["Off", "Low", "High", "Max"]} onCommit={commit} />);
  const knob = screen.getByRole("slider");
  fireEvent.keyDown(knob, { key: "ArrowUp" }); fireEvent.keyDown(knob, { key: "ArrowRight" });
  expect(commit).not.toHaveBeenCalled(); expect(knob.getAttribute("aria-valuetext")).toBe("High");
  act(() => { vi.advanceTimersByTime(200); });
  fireEvent.wheel(knob, { deltaY: -10 });
  wait(); expect(commit).toHaveBeenCalledExactlyOnceWith(1);
});

it("handles range limits, home/end, page step, reset and escape", () => {
  const commit = vi.fn();
  render(<Knob label="Amount" value={.5} defaultValue={.2} onCommit={commit} />);
  const knob = screen.getByRole("slider");
  fireEvent.keyDown(knob, { key: "PageUp" }); wait(); expect(commit).toHaveBeenLastCalledWith(.6);
  fireEvent.keyDown(knob, { key: "End" }); fireEvent.keyDown(knob, { key: "ArrowUp" }); wait(); expect(commit).toHaveBeenLastCalledWith(1);
  fireEvent.keyDown(knob, { key: "Home" }); fireEvent.keyDown(knob, { key: "ArrowDown" }); wait(); expect(commit).toHaveBeenLastCalledWith(0);
  fireEvent.doubleClick(knob); expect(commit).toHaveBeenLastCalledWith(.2);
  commit.mockClear(); fireEvent.keyDown(knob, { key: "End" }); fireEvent.keyDown(knob, { key: "Escape" }); wait();
  expect(commit).not.toHaveBeenCalled(); expect(knob.getAttribute("aria-valuenow")).toBe("50");
});

it("captures a drag, commits only on release and applies Shift to incremental deltas", () => {
  const commit = vi.fn();
  render(<Knob label="Amount" value={.5} onCommit={commit} />);
  const knob = screen.getByRole("slider");
  fireEvent.pointerDown(knob, { clientY: 200, button: 0 });
  fireEvent.pointerMove(knob, { clientY: 160 });
  fireEvent.pointerMove(knob, { clientY: 140, shiftKey: true });
  expect(commit).not.toHaveBeenCalled();
  fireEvent.pointerUp(knob, { clientY: 140 });
  expect(commit).toHaveBeenCalledExactlyOnceWith(.71);
});

it("does not cancel the captured pointer when another touch ends", () => {
  const commit = vi.fn(); render(<Knob label="Amount" value={.5} onCommit={commit} />);
  const knob = screen.getByRole("slider");
  fireEvent.pointerDown(knob, { pointerId: 1, clientY: 200, button: 0 });
  fireEvent.pointerMove(knob, { pointerId: 1, clientY: 180 });
  fireEvent.pointerCancel(knob, { pointerId: 2 });
  fireEvent.lostPointerCapture(knob, { pointerId: 2 });
  fireEvent.pointerUp(knob, { pointerId: 1, clientY: 180 });
  expect(commit).toHaveBeenCalledExactlyOnceWith(.6);
});

it("cancels interrupted drags and pending commits when disabled or unmounted", () => {
  const commit = vi.fn();
  const view = render(<Knob label="Amount" value={.5} onCommit={commit} />);
  const knob = screen.getByRole("slider");
  fireEvent.pointerDown(knob, { clientY: 200, button: 0 }); fireEvent.pointerMove(knob, { clientY: 100 }); fireEvent.pointerCancel(knob);
  expect(commit).not.toHaveBeenCalled();
  fireEvent.keyDown(knob, { key: "End" });
  view.rerender(<Knob label="Amount" value={.5} onCommit={commit} disabled />); wait();
  expect(commit).not.toHaveBeenCalled();
  view.rerender(<Knob label="Amount" value={.5} onCommit={commit} />);
  fireEvent.keyDown(knob, { key: "End" }); view.unmount(); wait();
  expect(commit).not.toHaveBeenCalled();
});

it("keeps a pending edit across unrelated renders with equivalent detent arrays", () => {
  const commit = vi.fn();
  const view = render(<Knob label="Thinking" value={0} detents={["Low", "High"]} onCommit={commit} />);
  fireEvent.keyDown(screen.getByRole("slider"), { key: "End" });
  view.rerender(<Knob label="Thinking" value={0} detents={["Low", "High"]} onCommit={commit} />);
  wait(); expect(commit).toHaveBeenCalledExactlyOnceWith(1);
});

it("lets the visual detent snap finish when the parent acknowledges the committed value", () => {
  function Controlled() {
    const [value, setValue] = useState(0);
    return <Knob label="Thinking" value={value} detents={["Low", "Medium", "High", "Max"]} onCommit={setValue} />;
  }
  const view = render(<Controlled />);
  const knob = screen.getByRole("slider");
  fireEvent.pointerDown(knob, { clientY: 200, button: 0 });
  fireEvent.pointerMove(knob, { clientY: 88 });
  act(() => { vi.advanceTimersByTime(20); });
  fireEvent.pointerUp(knob, { clientY: 88 });
  expect(view.container.querySelector(".synth-knob__arc")?.getAttribute("d")).not.toBe(arc(26, 23, 2 / 3));
  act(() => { vi.advanceTimersByTime(250); });
  expect(view.container.querySelector(".synth-knob__arc")?.getAttribute("d")).toBe(arc(26, 23, 2 / 3));
});

it("readout meters and single/disabled levels are never editable", () => {
  const commit = vi.fn();
  const view = render(<Knob label="Covered" value={.4} readOnly onCommit={commit} />);
  const meter = screen.getByRole("meter");
  expect(meter.getAttribute("tabindex")).toBeNull();
  fireEvent.wheel(meter, { deltaY: -1 }); fireEvent.doubleClick(meter); fireEvent.keyDown(meter, { key: "End" }); wait();
  expect(commit).not.toHaveBeenCalled();
  view.rerender(<Knob label="Thinking" value={0} detents={["Only"]} onCommit={commit} />);
  expect(screen.getByRole("slider").getAttribute("aria-disabled")).toBe("true");
});
