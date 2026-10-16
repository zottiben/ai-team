import { afterEach, expect, it, vi } from "vitest";
import { finiteMotion, inViewport, type FiniteTarget } from "./finite";

const original = Object.getOwnPropertyDescriptor(Element.prototype, "animate");
afterEach(() => {
  document.dispatchEvent(new Event("pointerdown")); document.body.replaceChildren(); vi.restoreAllMocks(); vi.unstubAllGlobals();
  if (original) Object.defineProperty(Element.prototype, "animate", original); else Reflect.deleteProperty(Element.prototype, "animate");
});
function fixture() {
  const owner = document.createElement("div"), element = document.createElement("button"); owner.append(element); document.body.append(owner);
  let box = new DOMRect(30, 40, 50, 20);
  vi.spyOn(owner, "getBoundingClientRect").mockReturnValue(new DOMRect(20, 30, 500, 300));
  vi.spyOn(element, "getBoundingClientRect").mockImplementation(() => box);
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([box] as unknown as DOMRectList);
  const animations: { id: string; onfinish: (() => void) | null; oncancel: (() => void) | null; cancel: ReturnType<typeof vi.fn> }[] = [];
  const animate = vi.fn(() => { const animation = { id: "", onfinish: null, oncancel: null, cancel: vi.fn() }; animations.push(animation); return animation; });
  Object.defineProperty(Element.prototype, "animate", { configurable: true, value: animate });
  const target: FiniteTarget = { element, frames: [{ opacity: 0, translate: "0 8px" }, { opacity: 1, translate: "0 0" }], duration: 180, kind: "receipt" };
  return { owner, element, target, animate, animation: (index: number) => { const found = animations[index]; if (!found) throw Error(`Missing synthetic animation ${index}`); return found; }, move: (next: DOMRect) => { box = next; } };
}
it("runs one finite effect without touching scroll or focus and cleans up on completion", () => {
  const f = fixture(); f.element.focus(); f.owner.scrollTop = 20;
  const cancel = finiteMotion(f.owner, [f.target]);
  expect(f.animate).toHaveBeenCalledWith(f.target.frames, { duration: 180, easing: "ease-out", iterations: 1, fill: "none" });
  expect(f.animation(0).id).toBe("synth-receipt-arrival"); expect(document.activeElement).toBe(f.element); expect(f.owner.scrollTop).toBe(20);
  f.animation(0).onfinish?.(); cancel(); document.dispatchEvent(new Event("wheel"));
  expect(f.animation(0).cancel).toHaveBeenCalledTimes(1); expect(f.element.style.opacity).toBe("");
});
it("refuses hidden, clipped, reduced and unsupported effects without a replay queue", () => {
  const f = fixture(); f.owner.hidden = true; finiteMotion(f.owner, [f.target]); f.owner.hidden = false;
  f.move(new DOMRect(600, 40, 40, 20)); expect(inViewport(f.element, f.owner)).toBe(false); finiteMotion(f.owner, [f.target]);
  f.move(new DOMRect(30, 40, 50, 20)); vi.stubGlobal("matchMedia", () => ({ matches: true })); finiteMotion(f.owner, [f.target]); vi.unstubAllGlobals();
  finiteMotion(f.owner, [{ ...f.target, duration: Infinity }, { ...f.target, duration: 601 }]);
  expect(f.animate).not.toHaveBeenCalled(); Reflect.deleteProperty(Element.prototype, "animate"); finiteMotion(f.owner, [f.target]);
});
it("ignores an unchanged owned scroll but cancels clipped or newer input ownership", () => {
  const f = fixture(); finiteMotion(f.owner, [f.target]); f.owner.dispatchEvent(new Event("scroll")); expect(f.animation(0).cancel).not.toHaveBeenCalled();
  f.move(new DOMRect(30, 800, 50, 20)); f.owner.dispatchEvent(new Event("scroll")); expect(f.animation(0).cancel).toHaveBeenCalledTimes(1);
  f.move(new DOMRect(30, 40, 50, 20)); finiteMotion(f.owner, [f.target]); document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab" })); expect(f.animation(1).cancel).toHaveBeenCalledTimes(1);
  finiteMotion(f.owner, [f.target]); f.owner.hidden = true; f.owner.dispatchEvent(new Event("scroll")); expect(f.animation(2).cancel).toHaveBeenCalledTimes(1);
});
it("stops immediately on real document hiding and changed motion preference", () => {
  const f = fixture(), events = new EventTarget();
  const media = { matches: false, addEventListener: events.addEventListener.bind(events), removeEventListener: events.removeEventListener.bind(events) };
  vi.stubGlobal("matchMedia", () => media); finiteMotion(f.owner, [f.target]); media.matches = true; events.dispatchEvent(new Event("change")); expect(f.animation(0).cancel).toHaveBeenCalledTimes(1);
  media.matches = false; finiteMotion(f.owner, [f.target]); vi.spyOn(document, "hidden", "get").mockReturnValue(true); document.dispatchEvent(new Event("visibilitychange")); expect(f.animation(1).cancel).toHaveBeenCalledTimes(1);
});
it("bounds simultaneous effects, deduplicates targets and falls back if animation throws", () => {
  const f = fixture(); finiteMotion(f.owner, [f.target, f.target]); expect(f.animate).toHaveBeenCalledTimes(1); document.dispatchEvent(new Event("pointerdown"));
  const many = Array.from({ length: 40 }, () => { const element = document.createElement("button"); f.owner.append(element); vi.spyOn(element, "getBoundingClientRect").mockReturnValue(new DOMRect(30, 40, 50, 20)); return { ...f.target, element }; });
  f.animate.mockClear(); const cancel = finiteMotion(f.owner, many); expect(f.animate).toHaveBeenCalledTimes(32); cancel();
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {}); f.animate.mockImplementation(() => { throw Error("Synthetic WAAPI failure"); });
  finiteMotion(f.owner, [f.target]); expect(warn).toHaveBeenCalledTimes(1); expect(f.element.isConnected).toBe(true);
});
