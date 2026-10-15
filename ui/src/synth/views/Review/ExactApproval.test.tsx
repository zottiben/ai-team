import { useRef } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ExactApproval } from "./ExactApproval";
import { setSynth } from "../../../ui-preferences";
import type { PreviewEvidence, PreviewFamily } from "../../../preview-api";

const api = vi.hoisted(() => ({ previewEvidence: vi.fn() }));
vi.mock("../../../preview-api", () => api);
const approve = vi.fn(), dismiss = vi.fn();
type Receipt = { id: number; chat_id: number; rev: number; evidence?: PreviewEvidence | null };
const receipt = (): Receipt => ({ id: 9, chat_id: 4, rev: 1 });
function evidence(text: string): PreviewEvidence {
  return { basis: "Pinned fixture range", files_total: 1, omitted_files: 0, note: null, files: [{ area: "commit", revision: "pinned", reason: null, hunks_omitted: false,
    file: { path: "source.ts", old_path: null, status: "added", binary: false, additions: 1, deletions: 0,
      hunks: [{ header: "@@ -0,0 +1 @@", old_start: 0, new_start: 1, lines: [{ kind: "added", old: null, new: 1, text }] }] } }] };
}
function deferred<T>() {
  let resolve: (value: T) => void = () => { throw Error("unarmed promise"); };
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve: (value: T) => resolve(value) };
}
function Harness({ value, family = "checkout" }: { value: Receipt | null; family?: PreviewFamily }) {
  const host = useRef<HTMLElement>(null);
  return <section ref={host} tabIndex={-1} aria-label="Controller"><input aria-label="Another draft"/>
    {value && <ExactApproval family={family} receipt={value} label="Fixture approval" returnFocus={host}
      summary={<p>Snapshot {value.id} for chat {value.chat_id}</p>}>
      {ready => <><button disabled={!ready} onClick={approve}>Approve fixture</button><button onClick={dismiss}>Discard fixture</button></>}
    </ExactApproval>}
  </section>;
}
beforeEach(() => { localStorage.clear(); act(() => setSynth(true)); vi.clearAllMocks(); api.previewEvidence.mockReset().mockResolvedValue(null); });
afterEach(() => { act(() => setSynth(false)); });
it("reads one exact receipt and retains that request across closing, reopening and skins", async () => {
  const pending = deferred<PreviewEvidence | null>(); api.previewEvidence.mockReturnValue(pending.promise);
  render(<Harness value={receipt()}/>);
  const dialog = await screen.findByRole("dialog", { name: "Fixture approval" });
  expect(api.previewEvidence).toHaveBeenCalledWith(4, "checkout", 9);
  expect((within(dialog).getByRole("button", { name: "Approve fixture" }) as HTMLButtonElement).disabled).toBe(true);
  expect((within(dialog).getByRole("button", { name: "Discard fixture" }) as HTMLButtonElement).disabled).toBe(false);
  fireEvent.keyDown(document, { key: "Escape" });
  await act(async () => pending.resolve(evidence("captured once")));
  fireEvent.click(screen.getByRole("button", { name: "Inspect captured checkout approval" }));
  await screen.findByText("captured once");
  expect(api.previewEvidence).toHaveBeenCalledTimes(1);
  act(() => setSynth(false)); act(() => setSynth(true));
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Inspect captured checkout approval" }));
  expect(await screen.findByText("captured once")).toBeTruthy();
  expect(api.previewEvidence).toHaveBeenCalledTimes(1);
  expect(approve).not.toHaveBeenCalled();
});
it("discards late evidence from another chat or receipt family", async () => {
  const old = deferred<PreviewEvidence | null>(), current = deferred<PreviewEvidence | null>();
  api.previewEvidence.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
  const view = render(<Harness value={receipt()}/>);
  await screen.findByRole("dialog");
  view.rerender(<Harness value={{ ...receipt(), chat_id: 5 }} family="delivery"/>);
  const dialog = await screen.findByRole("dialog");
  await act(async () => old.resolve(evidence("wrong receipt")));
  expect(within(dialog).queryByText("wrong receipt")).toBeNull();
  expect((within(dialog).getByRole("button", { name: "Approve fixture" }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => current.resolve(evidence("current receipt")));
  expect(within(dialog).getByText("current receipt")).toBeTruthy();
  expect(api.previewEvidence.mock.calls).toEqual([[4, "checkout", 9], [5, "delivery", 9]]);
  expect(approve).not.toHaveBeenCalled();
});
it("keeps missing history unknown and requires an explicit retry after a read failure", async () => {
  api.previewEvidence.mockRejectedValueOnce(Error("read unavailable")).mockResolvedValueOnce(null);
  render(<Harness value={receipt()}/>);
  const dialog = await screen.findByRole("dialog");
  await within(dialog).findByText(/read unavailable/);
  expect((within(dialog).getByRole("button", { name: "Approve fixture" }) as HTMLButtonElement).disabled).toBe(true);
  expect(api.previewEvidence).toHaveBeenCalledTimes(1);
  fireEvent.click(within(dialog).getByRole("button", { name: "Retry captured evidence" }));
  await within(dialog).findByText(/No pinned code evidence was recorded/);
  expect(api.previewEvidence).toHaveBeenCalledTimes(2);
  expect(approve).not.toHaveBeenCalled();
});
it("does not float a late preview out of a hidden owner or open it on return", async () => {
  const view = render(<div hidden><Harness value={receipt()}/></div>);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(api.previewEvidence).not.toHaveBeenCalled();
  view.rerender(<div><Harness value={receipt()}/></div>);
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Inspect captured checkout approval" }));
  await screen.findByRole("dialog");
  await waitFor(() => expect(api.previewEvidence).toHaveBeenCalledTimes(1));
});
it("closes when the owning panel is hidden and does not reopen on its own", async () => {
  const value = { ...receipt(), evidence: evidence("captured") };
  const view = render(<div><Harness value={value}/></div>);
  await screen.findByRole("dialog");
  view.rerender(<div hidden><Harness value={value}/></div>);
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  view.rerender(<div><Harness value={value}/></div>);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(approve).not.toHaveBeenCalled();
});
it("keeps a visible confirmation when a refresh moves its trigger offscreen", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "IntersectionObserver");
  const observers: { target: Element; report: (visible: boolean) => void }[] = [];
  class Observer {
    constructor(private callback: (entries: { target: Element; isIntersecting: boolean }[]) => void) {}
    observe(target: Element) {
      const report = (isIntersecting: boolean) => this.callback([{ target, isIntersecting }]);
      observers.push({ target, report }); report(true);
    }
    disconnect() {}
  }
  Object.defineProperty(globalThis, "IntersectionObserver", { configurable: true, value: Observer });
  try {
    render(<Harness value={{ ...receipt(), evidence: evidence("captured") }}/>);
    await screen.findByRole("dialog");
    const host = screen.getByRole("region", { name: "Fixture approval" });
    const observer = observers.find(entry => entry.target === host);
    if (observer) act(() => observer.report(false));
    expect(screen.queryByRole("dialog")).not.toBeNull();
    expect(approve).not.toHaveBeenCalled();
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "IntersectionObserver", descriptor);
    else Reflect.deleteProperty(globalThis, "IntersectionObserver");
  }
});
it("restores owned focus when discarded without stealing a newer input's focus", async () => {
  const value = { ...receipt(), evidence: evidence("captured") };
  const view = render(<Harness value={value}/>);
  const dialog = await screen.findByRole("dialog");
  act(() => within(dialog).getByRole("button", { name: "Discard fixture" }).focus());
  view.rerender(<Harness value={null}/>);
  expect(document.activeElement).toBe(screen.getByRole("region", { name: "Controller" }));
  view.rerender(<Harness value={value}/>);
  await screen.findByRole("dialog");
  const input = screen.getByLabelText("Another draft");
  act(() => input.focus());
  view.rerender(<Harness value={null}/>);
  expect(document.activeElement).toBe(input);
  expect(approve).not.toHaveBeenCalled();
});
