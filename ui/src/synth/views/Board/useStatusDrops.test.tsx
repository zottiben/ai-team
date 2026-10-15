import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PlanSlice } from "../../../plan-api";
import type { SavePlan } from "../../../PlanForms";
import { useStatusDrops } from "./useStatusDrops";
import type { DropView, StatusDrop } from "./drop";

const roots: HTMLElement[] = [];
afterEach(() => { roots.splice(0).forEach(node => node.remove()); vi.restoreAllMocks(); });
const slice: PlanSlice = { id: 1, key: "S1", title: "Original", status: "ready", scope_md: "Scope", demo_md: "Demo", blocked_reason: null, rev: 1 };
const initial: DropView = { slices: [slice], revision: 4, disabled: false };
const drop: StatusDrop = { key: "S1", from: "ready", to: "active", revision: 4 };
function setup(save: SavePlan) {
  const host = document.createElement("div"); document.body.append(host); roots.push(host);
  const button = document.createElement("button"); button.dataset.sliceDrag = "S1"; button.scrollIntoView = vi.fn(); host.append(button); button.focus();
  const box = new DOMRect(0, 0, 240, 480);
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue(Object.assign([box], { item: (index: number) => index === 0 ? box : null }));
  const ref = { current: host }, announce = vi.fn(), effects = { confirmed: vi.fn(), settle: vi.fn() };
  const hook = renderHook(({ view, ready, active }) => useStatusDrops(view, active, save, ref, ready, effects), { initialProps: { view: initial, ready: true, active: true } });
  const propose = (value = drop) => hook.result.current.propose(value, button, true, announce, new KeyboardEvent("keydown", { code: "Space" }));
  return { ...hook, host, button, announce, propose, effects };
}
describe("captured status proposals", () => {
  it("sends the captured revision and waits for both the committed snapshot and re-enabled controls", async () => {
    let resolve!: (ok: boolean) => void;
    const save = vi.fn<SavePlan>().mockReturnValue(new Promise(done => { resolve = done; }));
    const test = setup(save);
    act(() => test.propose());
    test.rerender({ view: { ...initial, revision: 5 }, ready: false, active: true });
    await waitFor(() => expect(save).toHaveBeenCalledWith({ action: "set_slice_status", key: "S1", status: "active" }, 4));
    await act(async () => { resolve(true); }); expect(test.announce).not.toHaveBeenCalled();
    test.rerender({ view: { ...initial, revision: 5, slices: [{ ...slice, status: "active" }] }, ready: false, active: true });
    expect(test.announce).not.toHaveBeenCalled(); expect(test.effects.confirmed).not.toHaveBeenCalled();
    test.rerender({ view: { ...initial, revision: 5, slices: [{ ...slice, status: "active" }] }, ready: true, active: true });
    expect(test.announce).toHaveBeenCalledOnce(); expect(test.announce).toHaveBeenLastCalledWith("S1 recorded as active, plan position 1. Status only.");
    expect(test.effects.confirmed).toHaveBeenCalledExactlyOnceWith(drop);
    await waitFor(() => expect(test.effects.settle).toHaveBeenCalledExactlyOnceWith(drop, null));
    test.unmount();
  });
  it("never flashes a confirmed event or returns an already-changed card after an unknown response", async () => {
    let resolve!: (ok: boolean) => void;
    const save = vi.fn<SavePlan>().mockReturnValue(new Promise(done => { resolve = done; })), test = setup(save);
    act(() => test.propose()); await waitFor(() => expect(save).toHaveBeenCalledOnce());
    test.rerender({ view: { ...initial, revision: 5, slices: [{ ...slice, status: "active" }] }, ready: true, active: true });
    await act(async () => { resolve(false); });
    await waitFor(() => expect(test.button.scrollIntoView).toHaveBeenCalled());
    expect(test.effects.confirmed).not.toHaveBeenCalled(); expect(test.effects.settle).not.toHaveBeenCalled();
    expect(test.announce).toHaveBeenCalledWith("The status request was not confirmed. Review the current plan."); test.unmount();
  });
  it("a blocked drop is only a proposal; changed revisions and hidden owners cannot submit it", async () => {
    const save = vi.fn<SavePlan>().mockResolvedValue(false), test = setup(save);
    act(() => test.propose({ ...drop, to: "blocked" }));
    expect(test.result.current.proposal?.drop).toEqual({ ...drop, to: "blocked" }); expect(save).not.toHaveBeenCalled();
    const action = { action: "set_slice_status", key: "S1", status: "blocked", reason: "Original reason" } as const;
    test.host.hidden = true;
    expect(await test.result.current.saveBlocked(action, 4)).toBe(false); expect(save).not.toHaveBeenCalled();
    act(() => test.result.current.close(false)); expect(test.result.current.proposal?.drop).toEqual({ ...drop, to: "blocked" });
    test.host.hidden = false;
    test.rerender({ view: { ...initial, revision: 5 }, ready: false, active: true });
    expect(await test.result.current.saveBlocked(action, 4)).toBe(false); expect(save).not.toHaveBeenCalled();
    expect(test.result.current.proposal?.drop.revision).toBe(4);
    test.rerender({ view: initial, ready: false, active: true });
    expect(await test.result.current.saveBlocked({ ...action, key: "S2" }, 4)).toBe(false);
    expect(await test.result.current.saveBlocked(action, 5)).toBe(false); expect(save).not.toHaveBeenCalled();
    expect(await test.result.current.saveBlocked(action, 4)).toBe(false);
    expect(save).toHaveBeenCalledExactlyOnceWith(action, 4); test.unmount();
  });
  it("does not dispatch from a hidden owner or announce a late response into another view", async () => {
    let resolve!: (ok: boolean) => void;
    const save = vi.fn<SavePlan>().mockReturnValue(new Promise(done => { resolve = done; })), test = setup(save);
    test.rerender({ view: initial, ready: true, active: false }); act(() => test.propose()); expect(save).not.toHaveBeenCalled();
    test.rerender({ view: initial, ready: true, active: true }); act(() => test.propose());
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    test.rerender({ view: initial, ready: false, active: false });
    await act(async () => { resolve(true); });
    test.rerender({ view: { ...initial, revision: 5, slices: [{ ...slice, status: "active" }] }, ready: true, active: true });
    expect(test.announce).not.toHaveBeenCalled(); test.unmount();
  });
});
