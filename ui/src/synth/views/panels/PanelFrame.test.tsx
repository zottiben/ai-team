import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import PanelFrame, { panelBounds } from "./PanelFrame";

const keys = ["setPointerCapture", "releasePointerCapture", "hasPointerCapture"] as const;
const saved = keys.map(key => Object.getOwnPropertyDescriptor(HTMLElement.prototype, key));
beforeEach(() => {
  localStorage.clear(); vi.stubGlobal("innerWidth", 1280);
  const captured = new WeakMap<Element, number>();
  vi.stubGlobal("PointerEvent", class extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) { super(type, init); this.pointerId = init.pointerId ?? 1; }
  });
  Object.defineProperties(HTMLElement.prototype, {
    setPointerCapture: { configurable: true, value(this: Element, id: number) { captured.set(this, id); } },
    releasePointerCapture: { configurable: true, value(this: Element) { captured.delete(this); } },
    hasPointerCapture: { configurable: true, value(this: Element, id: number) { return captured.get(this) === id; } },
  });
});
afterEach(() => {
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
  keys.forEach((key, i) => { const descriptor = saved[i]; if (descriptor) Object.defineProperty(HTMLElement.prototype, key, descriptor); else Reflect.deleteProperty(HTMLElement.prototype, key); });
});
function Fixture({ active = true, panel = "overview" }: { active?: boolean; panel?: "overview" | "review" | "work" }) {
  const host = useRef<HTMLDivElement>(null);
  return <div ref={host}><textarea aria-label="Draft" /><PanelFrame host={host} active={active} panel={panel} /><aside id="chat-context">Panel</aside></div>;
}

it("clamps preferences to 360px, 60% viewport and remaining conversation space", () => {
  expect(panelBounds(1000, 1600, 1300)).toEqual({ width: 960, max: 960, stacked: false });
  expect(panelBounds(800, 1280, 900)).toEqual({ width: 568, max: 568, stacked: false });
  expect(panelBounds(0, 1280, 900).width).toBe(360);
  expect(panelBounds(NaN, 1280, 900).width).toBe(400);
  expect(panelBounds(400, 899, 899).stacked).toBe(true);
  expect(panelBounds(400, 900, 584).stacked).toBe(true);
});

it("uses the screen-specific defaults until the operator chooses a retained width", () => {
  const view = render(<Fixture />);
  expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("400");
  view.rerender(<Fixture panel="review" />);
  expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("660");
  view.rerender(<Fixture panel="work" />);
  expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("640");
  fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowRight" });
  view.rerender(<Fixture panel="overview" />);
  expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("630");
});

it("resizes by keyboard, persists a bounded width and keeps the draft node", () => {
  const view = render(<Fixture />), handle = screen.getByRole("separator");
  const draft = screen.getByRole("textbox", { name: "Draft" }); fireEvent.change(draft, { target: { value: "untouched" } });
  fireEvent.keyDown(handle, { key: "ArrowLeft" });
  expect(handle.getAttribute("aria-valuenow")).toBe("410");
  expect(localStorage.getItem("ui.synth.panel-width")).toBe("410");
  fireEvent.keyDown(handle, { key: "End" }); expect(handle.getAttribute("aria-valuenow")).toBe("768");
  fireEvent.keyDown(handle, { key: "Home" }); expect(handle.getAttribute("aria-valuenow")).toBe("360");
  view.rerender(<Fixture active={false} />);
  expect(screen.queryByRole("separator")).toBeNull();
  expect(screen.getByRole("textbox", { name: "Draft" })).toBe(draft);
  expect((draft as HTMLTextAreaElement).value).toBe("untouched");
  view.rerender(<Fixture />); expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("360");
});

it("draws a pointer drag without saving until release and cancels only its own pointer", () => {
  render(<Fixture />); const handle = screen.getByRole("separator");
  fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 700 });
  fireEvent.pointerMove(handle, { pointerId: 1, clientX: 650 });
  expect(handle.getAttribute("aria-valuenow")).toBe("450"); expect(localStorage.getItem("ui.synth.panel-width")).toBeNull();
  fireEvent.pointerCancel(handle, { pointerId: 2 });
  fireEvent.pointerUp(handle, { pointerId: 1, clientX: 650 });
  expect(localStorage.getItem("ui.synth.panel-width")).toBe("450");
  fireEvent.pointerDown(handle, { pointerId: 3, button: 0, clientX: 700 });
  fireEvent.pointerMove(handle, { pointerId: 3, clientX: 600 });
  fireEvent.keyDown(handle, { key: "Escape" });
  expect(handle.getAttribute("aria-valuenow")).toBe("450"); expect(handle.hasPointerCapture(3)).toBe(false);
  expect(localStorage.getItem("ui.synth.panel-width")).toBe("450");
});

it("stacks at narrow widths without destroying the saved desktop preference", () => {
  localStorage.setItem("ui.synth.panel-width", "700");
  const view = render(<Fixture />);
  vi.stubGlobal("innerWidth", 700); act(() => window.dispatchEvent(new Event("resize")));
  expect(screen.queryByRole("separator")).toBeNull();
  expect(view.container.firstElementChild?.getAttribute("data-synth-stacked")).toBe("true");
  expect(localStorage.getItem("ui.synth.panel-width")).toBe("700");
  vi.stubGlobal("innerWidth", 1280); act(() => window.dispatchEvent(new Event("resize")));
  expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("700");
  view.unmount();
});

it("uses a safe default for corrupt storage and still resizes when writes fail", () => {
  localStorage.setItem("ui.synth.panel-width", "not a width");
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Storage unavailable"); });
  render(<Fixture />); const handle = screen.getByRole("separator");
  expect(handle.getAttribute("aria-valuenow")).toBe("400");
  fireEvent.keyDown(handle, { key: "ArrowLeft" }); expect(handle.getAttribute("aria-valuenow")).toBe("410");
});
