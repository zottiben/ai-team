import { afterEach, describe, expect, it, vi } from "vitest";
import { cardPose, settleCard } from "./settle";

const originalAnimate = Object.getOwnPropertyDescriptor(Element.prototype, "animate");
afterEach(() => {
  document.dispatchEvent(new Event("pointerdown")); document.body.replaceChildren(); vi.restoreAllMocks(); vi.unstubAllGlobals();
  if (originalAnimate) Object.defineProperty(Element.prototype, "animate", originalAnimate); else Reflect.deleteProperty(Element.prototype, "animate");
});
function fixture() {
  const board = document.createElement("div"), lane = document.createElement("section"), card = document.createElement("article"), handle = document.createElement("button");
  board.dataset.motion = "true"; lane.className = "plan-lane"; lane.dataset.status = "active"; card.dataset.sliceKey = "S1"; card.className = "plan-slice-card";
  handle.id = "original-handle"; handle.textContent = "Move S1"; handle.scrollIntoView = vi.fn(); card.append(handle); lane.append(card); board.append(lane); document.body.append(board);
  let target = new DOMRect(250, 200, 220, 80);
  vi.spyOn(board, "getBoundingClientRect").mockReturnValue(new DOMRect(10, 50, 700, 500));
  vi.spyOn(card, "getBoundingClientRect").mockImplementation(() => target);
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([target] as unknown as DOMRectList);
  const animation = { onfinish: null as (() => void) | null, oncancel: null as (() => void) | null, cancel: vi.fn() };
  const animate = vi.fn(() => animation); Object.defineProperty(Element.prototype, "animate", { configurable: true, value: animate });
  const pose = cardPose(board, card)!; pose.box = { left: 50, top: 120, width: 220, height: 80 }; handle.focus();
  return { board, lane, card, handle, pose, animation, animate, move: () => { target = new DOMRect(260, 220, 220, 80); } };
}
describe("finite Board card settlement", () => {
  it("animates only an inert portal while retaining the original focus owner, then cleans up", () => {
    const f = fixture(), ended = vi.fn(); const effect = settleCard(f.board, "S1", "active", f.pose, ended);
    expect(effect).not.toBeNull(); expect(f.animate).toHaveBeenCalledWith([
      { transform: "translate(-200px, -83px) scale(1, 1) rotate(-1.5deg)" }, { transform: "none" },
    ], { duration: 200, easing: "ease-out" });
    const copy = document.querySelector<HTMLElement>(".synth-plan-settle-overlay")!;
    expect(copy.inert).toBe(true); expect(copy.getAttribute("aria-hidden")).toBe("true"); expect(copy.querySelector("[id]")).toBeNull();
    expect(copy.querySelector("button")?.tabIndex).toBe(-1); expect(document.activeElement).toBe(f.handle);
    expect(f.card.isConnected).toBe(true); expect(f.card.dataset.settling).toBeDefined();
    f.animation.onfinish?.();
    expect(ended).toHaveBeenCalledTimes(1); expect(f.animation.cancel).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".synth-plan-settle-layer")).toBeNull(); expect(f.card.dataset.settling).toBeUndefined(); expect(f.board.dataset.flipping).toBeUndefined();
    effect?.cancel(); expect(ended).toHaveBeenCalledTimes(1);
  });
  it("refuses changed-viewport, wrong-status, hidden and reduced-motion geometry without changing cards", () => {
    const f = fixture();
    expect(settleCard(f.board, "S1", "ready", f.pose)).toBeNull();
    expect(settleCard(f.board, "S1", "active", { ...f.pose, width: f.pose.width + 50 })).toBeNull();
    f.board.hidden = true; expect(settleCard(f.board, "S1", "active", f.pose)).toBeNull(); f.board.hidden = false;
    vi.stubGlobal("matchMedia", () => ({ matches: true }));
    expect(cardPose(f.board, f.card)).toBeNull(); expect(settleCard(f.board, "S1", "active", f.pose)).toBeNull();
    expect(f.animate).not.toHaveBeenCalled(); expect(f.card.dataset.settling).toBeUndefined(); expect(f.board.dataset.flipping).toBeUndefined();
  });
  it("ignores a delayed owned scroll event but retires when geometry or new input changes", () => {
    const f = fixture(), ended = vi.fn(); settleCard(f.board, "S1", "active", f.pose, ended);
    f.board.dispatchEvent(new Event("scroll")); expect(ended).not.toHaveBeenCalled();
    f.move(); f.board.dispatchEvent(new Event("scroll")); expect(ended).toHaveBeenCalledTimes(1);
    const next = cardPose(f.board, f.card)!; settleCard(f.board, "S1", "active", next, ended);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab" })); expect(ended).toHaveBeenCalledTimes(2);
    document.dispatchEvent(new Event("pointerdown")); expect(ended).toHaveBeenCalledTimes(2);
  });
  it("retires immediately when the actual motion preference changes", () => {
    const events = new EventTarget(), media = { matches: false, addEventListener: events.addEventListener.bind(events), removeEventListener: events.removeEventListener.bind(events) };
    vi.stubGlobal("matchMedia", () => media);
    const f = fixture(), ended = vi.fn(); settleCard(f.board, "S1", "active", f.pose, ended);
    media.matches = true; events.dispatchEvent(new Event("change"));
    expect(ended).toHaveBeenCalledTimes(1); expect(f.card.dataset.settling).toBeUndefined(); expect(document.querySelector(".synth-plan-settle-layer")).toBeNull();
  });
  it("fails to plain recorded state if animation is unavailable or throws", () => {
    const f = fixture(), warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    f.animate.mockImplementation(() => { throw Error("Synthetic animation failure"); });
    expect(settleCard(f.board, "S1", "active", f.pose)).toBeNull(); expect(warn).toHaveBeenCalledTimes(1);
    expect(f.card.dataset.settling).toBeUndefined(); expect(document.querySelector(".synth-plan-settle-layer")).toBeNull();
    Reflect.deleteProperty(Element.prototype, "animate"); expect(settleCard(f.board, "S1", "active", f.pose)).toBeNull();
    expect(f.board.dataset.flipping).toBeUndefined(); expect(document.activeElement).toBe(f.handle);
  });
});
