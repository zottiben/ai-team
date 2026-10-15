import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PlanStatusControl } from "./PlanStatusControl";

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([new DOMRect(0, 0, 200, 40)] as unknown as DOMRectList);
});
afterEach(() => vi.restoreAllMocks());
const announcement = () => document.querySelector('[aria-live="polite"]')?.textContent;
it("announces refusal only after response, with no optimistic status or duplicate write", async () => {
  let finish!: (ok: boolean) => void;
  const save = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; }));
  render(<PlanStatusControl status="ready" revision={4} disabled={false} save={save}/>);
  const next = screen.getByRole("button", { name: "Next Plan status" });
  screen.getByRole("button", { name: "Plan status" }).scrollIntoView = vi.fn();
  fireEvent.click(next); fireEvent.click(next);
  expect(save).toHaveBeenCalledExactlyOnceWith({ action: "set_plan_status", status: "active" }, 4);
  expect(screen.getByRole("button", { name: "Plan status" }).textContent).toBe("ready");
  expect(announcement()).toBe("");
  await act(async () => finish(false));
  expect(announcement()).toContain("not confirmed");
  expect((next as HTMLButtonElement).disabled).toBe(false);
});
it("closes a hidden owner's portal and refuses hidden or frozen input", async () => {
  const save = vi.fn().mockResolvedValue(true);
  const fixture = (hidden: boolean, disabled = false) => <div hidden={hidden}><PlanStatusControl status="ready" revision={4} disabled={disabled} save={save}/></div>;
  const view = render(fixture(false));
  const anchor = screen.getByRole("button", { name: "Plan status" }), next = screen.getByRole("button", { name: "Next Plan status" });
  fireEvent.click(anchor); expect(screen.getByRole("listbox")).toBeTruthy();
  view.rerender(fixture(true)); fireEvent.click(next);
  await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
  expect(save).not.toHaveBeenCalled();
  view.rerender(fixture(false, true)); fireEvent.click(next);
  expect(save).not.toHaveBeenCalled();
  expect((anchor as HTMLButtonElement).disabled).toBe(true);
});
it("does not restore selector focus over newer input after an accepted request", async () => {
  let finish!: (ok: boolean) => void;
  const save = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; }));
  render(<><PlanStatusControl status="ready" revision={4} disabled={false} save={save}/><input aria-label="Newer input"/></>);
  fireEvent.click(screen.getByRole("button", { name: "Next Plan status" }));
  const input = screen.getByRole("textbox", { name: "Newer input" }); input.focus();
  await act(async () => finish(true));
  await act(async () => { await new Promise(requestAnimationFrame); });
  expect(document.activeElement).toBe(input);
});
it("does not announce a response into a hidden owner", async () => {
  let finish!: (ok: boolean) => void;
  const save = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; }));
  const fixture = (hidden: boolean) => <div hidden={hidden}><PlanStatusControl status="ready" revision={4} disabled={false} save={save}/></div>;
  const view = render(fixture(false)); fireEvent.click(screen.getByRole("button", { name: "Next Plan status" }));
  view.rerender(fixture(true)); await act(async () => finish(true));
  expect(announcement()).toBe("");
  view.rerender(fixture(false)); expect(announcement()).toBe("");
});
