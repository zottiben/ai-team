import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PatchDemo } from "./PatchDemo";
import { SynthProvider } from "./SynthProvider";
import { patchPoint } from "./primitives/patch";

const methods = ["setPointerCapture", "releasePointerCapture", "hasPointerCapture"] as const;
const saved = methods.map(key => Object.getOwnPropertyDescriptor(HTMLElement.prototype, key));
const hitTest = Object.getOwnPropertyDescriptor(document, "elementFromPoint");
beforeEach(() => {
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
  cleanup(); vi.unstubAllGlobals();
  methods.forEach((key, i) => { const descriptor = saved[i]; if (descriptor) Object.defineProperty(HTMLElement.prototype, key, descriptor); else Reflect.deleteProperty(HTMLElement.prototype, key); });
  if (hitTest) Object.defineProperty(document, "elementFromPoint", hitTest); else Reflect.deleteProperty(document, "elementFromPoint");
});
const demo = () => render(<SynthProvider><PatchDemo /></SynthProvider>);

it("supports keyboard arming, fan-out, replacement and unplug without any runtime call", async () => {
  const user = userEvent.setup(); demo();
  screen.getByRole("button", { name: "Source A" }).focus(); await user.keyboard("{Enter}");
  await user.click(screen.getByRole("button", { name: "Input X" }));
  expect(screen.getByText("Source A → Input X")).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Source A" })); await user.click(screen.getByRole("button", { name: "Input Y" }));
  expect(screen.getByText("2 demo connections")).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Source B" })); await user.click(screen.getByRole("button", { name: "Input X" }));
  expect(screen.queryByText("Source A → Input X")).toBeNull(); expect(screen.getByText("Source B → Input X")).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Unplug Input X" }));
  expect(screen.getByText("1 demo connections")).toBeDefined(); expect(screen.getByText("Source A → Input Y")).toBeDefined();
});

it("rejects incompatible ports and cancels arming on Escape or empty space", async () => {
  const user = userEvent.setup(), view = demo();
  await user.click(screen.getByRole("button", { name: "Source A" }));
  await user.click(screen.getByRole("button", { name: "Verdict only" }));
  expect(screen.getByText("0 demo connections")).toBeDefined();
  expect(document.querySelector('[aria-live="polite"]')?.textContent).toContain("Incompatible");
  await user.keyboard("{Escape}"); expect(screen.getByRole("button", { name: "Source A" }).getAttribute("aria-pressed")).toBe("false");
  await user.click(screen.getByRole("button", { name: "Source A" }));
  fireEvent.pointerDown(view.container.querySelector(".synth-patch")!, { pointerId: 2 });
  expect(screen.getByRole("button", { name: "Source A" }).getAttribute("aria-pressed")).toBe("false");
});

it("captures only a real drag, not an ordinary click, and scopes the drop target", () => {
  const view = demo(), host = view.container.querySelector<HTMLElement>(".synth-patch")!;
  const source = screen.getByRole("button", { name: "Source A" }), input = screen.getByRole("button", { name: "Input X" });
  fireEvent.pointerDown(source, { pointerId: 1, button: 0, clientX: 10, clientY: 10 });
  expect(host.hasPointerCapture(1)).toBe(false);
  fireEvent.pointerMove(host, { pointerId: 1, clientX: 20, clientY: 20 }); expect(host.hasPointerCapture(1)).toBe(true);
  Object.defineProperty(document, "elementFromPoint", { configurable: true, value: () => input.firstElementChild });
  fireEvent.pointerUp(host, { pointerId: 1, clientX: 20, clientY: 100 });
  expect(screen.getByText("Source A → Input X")).toBeDefined(); expect(host.hasPointerCapture(1)).toBe(false);
  fireEvent.pointerDown(source, { pointerId: 2, button: 0, clientX: 10, clientY: 10 });
  fireEvent.pointerMove(host, { pointerId: 2, clientX: 30, clientY: 30 });
  fireEvent.pointerCancel(host, { pointerId: 2 });
  expect(screen.getByText("1 demo connections")).toBeDefined(); expect(host.hasPointerCapture(2)).toBe(false);
});

it("converts viewport coordinates through zoom and scroll offsets", () => {
  expect(patchPoint({ x: 220, y: 140 }, { left: 20, top: 40, width: 400, height: 200 }, 200, 100)).toEqual({ x: 100, y: 50 });
  expect(patchPoint({ x: 220, y: 40 }, { left: 20, top: -60, width: 400, height: 200 }, 200, 100)).toEqual({ x: 100, y: 50 });
});
