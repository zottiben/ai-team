import { act, render } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { useReceiptMotion, usePatternMotion } from "./useReceiptMotion";
import type { TrackerRow } from "./Tracker/rows";

const original = Object.getOwnPropertyDescriptor(Element.prototype, "animate");
afterEach(() => { vi.restoreAllMocks(); if (original) Object.defineProperty(Element.prototype, "animate", original); else Reflect.deleteProperty(Element.prototype, "animate"); });
function rows(ids: number[]): TrackerRow[] { return ids.map(id => ({ kind: "event", id, event: { id, actor: "assistant", node_run_id: 1, kind: "note", message: `Receipt ${id}`, summary: "Fixture", thinking: [], at: "coarse" } })); }
function Host({ ids, enabled = true, ready = true }: { ids: number[]; enabled?: boolean; ready?: boolean }) {
  const viewport = useRef<HTMLDivElement>(null), list = useRef<HTMLDivElement>(null);
  useReceiptMotion(rows(ids), list, viewport, enabled, undefined, ready);
  return <div ref={viewport} hidden={!enabled}><div ref={list}>{ids.map(id => <div key={id} data-row-id={id}>Receipt {id}</div>)}</div></div>;
}
function LoadedPattern({ ids, ready = true }: { ids: number[]; ready?: boolean }) {
  const viewport = useRef<HTMLDivElement>(null);
  usePatternMotion(rows(ids), viewport, 0, 48, ready);
  return <div ref={viewport}>{ids.map(id => <div data-pattern-id={id} key={id}><button><span/></button></div>)}</div>;
}
function fixture() {
  const box = new DOMRect(10, 10, 300, 200);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(box);
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([box] as unknown as DOMRectList);
  const cancel = vi.fn(), animate = vi.fn(() => ({ id: "", onfinish: null, oncancel: null, cancel }));
  Object.defineProperty(Element.prototype, "animate", { configurable: true, value: animate });
  return { cancel, animate };
}
it("animates a newly mounted receipt once, not initial history or a replay", async () => {
  const f = fixture(), view = render(<Host ids={[1]}/>); await act(async () => {}); expect(f.animate).not.toHaveBeenCalled();
  await act(async () => view.rerender(<Host ids={[1, 2]}/>)); expect(f.animate).toHaveBeenCalledTimes(1);
  await act(async () => view.rerender(<Host ids={[1, 2]}/>)); expect(f.animate).toHaveBeenCalledTimes(1); expect(f.cancel).toHaveBeenCalledTimes(1);
  await act(async () => view.rerender(<Host ids={[1]}/>)); await act(async () => view.rerender(<Host ids={[1, 2]}/>)); expect(f.animate).toHaveBeenCalledTimes(1);
});
it.each(["receipts", "pattern"])("consumes initial history but animates a later first receipt in a known-empty chat (%s)", async kind => {
  const View = kind === "receipts" ? Host : LoadedPattern;
  const f = fixture(), view = render(<View ids={[]} ready={false}/>); await act(async () => {});
  await act(async () => view.rerender(<View ids={[1, 2]}/>)); expect(f.animate).not.toHaveBeenCalled();
  await act(async () => view.rerender(<View ids={[1, 2, 3]}/>)); expect(f.animate).toHaveBeenCalledTimes(1);
  await act(async () => view.rerender(<View ids={[]} ready={false}/>));
  await act(async () => view.rerender(<View ids={[1]}/>));
  await act(async () => view.rerender(<View ids={[1, 2]}/>)); expect(f.animate).toHaveBeenCalledTimes(1);
  view.unmount(); f.animate.mockClear();
  const empty = render(<View ids={[]}/>); await act(async () => {});
  await act(async () => empty.rerender(<View ids={[1]}/>)); expect(f.animate).toHaveBeenCalledTimes(1);
});

it("shifts retained columns but does not push the newly popped column outside the viewport", async () => {
  const f = fixture();
  vi.mocked(HTMLElement.prototype.getBoundingClientRect).mockImplementation(function(this: HTMLElement) {
    const id = Number(this.closest<HTMLElement>("[data-pattern-id]")?.dataset.patternId);
    return id ? new DOMRect(id < 47 ? -100 : (id - 47) * 44 + 10, 10, 44, 200) : new DOMRect(0, 0, 300, 300);
  });
  function Pattern({ count }: { count: number }) {
    const viewport = useRef<HTMLDivElement>(null), ids = Array.from({ length: count }, (_, index) => index + 1);
    usePatternMotion(rows(ids), viewport, Math.max(0, count - 48), 48);
    return <div ref={viewport}>{ids.slice(-48).map(id => <div data-pattern-id={id} key={id}><button><span/></button></div>)}</div>;
  }
  const view = render(<Pattern count={48}/>); await act(async () => {});
  await act(async () => view.rerender(<Pattern count={49}/>));
  const added = view.container.querySelector('[data-pattern-id="49"]');
  expect(f.animate.mock.contexts).toContain(added?.querySelector("button > span"));
  expect(f.animate.mock.contexts).toContain(view.container.querySelector('[data-pattern-id="48"]'));
  expect(f.animate.mock.contexts).not.toContain(added);
});

it("consumes hidden arrivals without replaying on reveal and cancels old ownership", async () => {
  const f = fixture(), view = render(<Host ids={[1]} enabled={false}/>); await act(async () => {});
  await act(async () => view.rerender(<Host ids={[1, 2]} enabled={false}/>)); await act(async () => view.rerender(<Host ids={[1, 2]}/>)); expect(f.animate).not.toHaveBeenCalled();
  await act(async () => view.rerender(<Host ids={[1, 2, 3]}/>)); expect(f.animate).toHaveBeenCalledTimes(1);
  view.unmount(); expect(f.cancel).toHaveBeenCalledTimes(1);
});
