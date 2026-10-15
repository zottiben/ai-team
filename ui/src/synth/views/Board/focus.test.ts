import { afterEach, describe, expect, it, vi } from "vitest";
import { ownedFocus } from "./focus";

afterEach(() => { document.body.replaceChildren(); vi.restoreAllMocks(); vi.useRealTimers(); });
function fixture() {
  vi.useFakeTimers();
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([new DOMRect(0, 0, 40, 40)] as unknown as DOMRectList);
  const source = document.createElement("button"), target = document.createElement("button"); target.scrollIntoView = vi.fn(); document.body.append(source, target); source.focus();
  return { source, target, ticket: ownedFocus(node => node === source || node === target) };
}
describe("owned gesture focus handoff", () => {
  it("runs settlement only after the real target acquired focus", () => {
    const { ticket, target } = fixture(), restored = vi.fn(); ticket.restore(() => target, restored);
    expect(restored).not.toHaveBeenCalled(); vi.advanceTimersByTime(20);
    expect(document.activeElement).toBe(target); expect(restored).toHaveBeenCalledTimes(1);
  });
  it("does not start feedback on a still-disabled target", () => {
    const { ticket, source, target } = fixture(), restored = vi.fn(); target.disabled = true;
    ticket.restore(() => target, restored); vi.advanceTimersByTime(20);
    expect(document.activeElement).toBe(source); expect(restored).not.toHaveBeenCalled();
  });
  it("retires the ticket even when a presentation callback fails", () => {
    const { ticket, target } = fixture(); ticket.restore(() => target, () => { throw Error("Synthetic presentation failure"); });
    expect(() => vi.advanceTimersByTime(20)).toThrow("Synthetic presentation failure");
    const again = vi.fn(); ticket.restore(() => target, again); vi.advanceTimersByTime(20); expect(again).not.toHaveBeenCalled();
  });
  it.each(["wheel", "touchmove", "resize"])("does not take focus back after newer %s intent", event => {
    const { ticket, source, target } = fixture(), restored = vi.fn(); ticket.restore(() => target, restored);
    (event === "resize" ? window : document).dispatchEvent(new Event(event)); vi.advanceTimersByTime(20);
    expect(document.activeElement).toBe(source); expect(restored).not.toHaveBeenCalled();
  });
});
