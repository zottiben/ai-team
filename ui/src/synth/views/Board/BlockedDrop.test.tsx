import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { BlockedDrop } from "./BlockedDrop";
import type { StatusDrop } from "./drop";
const proposal: StatusDrop = { key: "S1", from: "ready", to: "blocked", revision: 4 };
it("requires a reason and sends only the captured status command", async () => {
  const save = vi.fn(async () => false), close = vi.fn();
  render(<BlockedDrop proposal={proposal} revision={4} disabled={false} save={save} close={close}/>);
  fireEvent.submit(screen.getByRole("form")); expect(save).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Blocking reason"), { target: { value: "Need a decision" } });
  fireEvent.submit(screen.getByRole("form"));
  await screen.findByText(/status change was not confirmed/);
  expect(save).toHaveBeenCalledExactlyOnceWith({ action: "set_slice_status", key: "S1", status: "blocked", reason: "Need a decision" }, 4);
  expect((screen.getByLabelText("Blocking reason") as HTMLInputElement).value).toBe("Need a decision");
  expect(close).not.toHaveBeenCalled();
});
it("retains the original target and draft but refuses stale or read-only submission", () => {
  const save = vi.fn(async () => true), close = vi.fn();
  const view = render(<BlockedDrop proposal={proposal} revision={4} disabled={false} save={save} close={close}/>);
  const input = screen.getByLabelText("Blocking reason"); fireEvent.change(input, { target: { value: "Original reason" } });
  view.rerender(<BlockedDrop proposal={{ ...proposal, key: "S2", revision: 5 }} revision={5} disabled={false} save={save} close={close}/>);
  expect(screen.getByRole("form", { name: "Blocking reason for S1" })).toBeTruthy();
  expect(screen.getByLabelText("Blocking reason")).toBe(input);
  expect((input as HTMLInputElement).value).toBe("Original reason");
  fireEvent.submit(screen.getByRole("form")); expect(save).not.toHaveBeenCalled();
  view.rerender(<BlockedDrop proposal={proposal} revision={4} disabled save={save} close={close}/>);
  expect((input as HTMLInputElement).readOnly).toBe(true);
  fireEvent.submit(screen.getByRole("form")); expect(save).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel status change" })); expect(close).toHaveBeenCalledOnce();
});
it("does not turn cancellation or a second submit into another in-flight request", async () => {
  let finish!: (value: boolean) => void;
  const save = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; })), close = vi.fn();
  render(<BlockedDrop proposal={proposal} revision={4} disabled={false} save={save} close={close}/>);
  const input = screen.getByLabelText("Blocking reason"); fireEvent.change(input, { target: { value: "Captured reason" } });
  fireEvent.submit(screen.getByRole("form")); fireEvent.submit(screen.getByRole("form"));
  fireEvent.keyDown(input, { key: "Escape" }); fireEvent.click(screen.getByRole("button", { name: "Cancel status change" }));
  expect(save).toHaveBeenCalledOnce(); expect(close).not.toHaveBeenCalled();
  await act(async () => finish(true)); await waitFor(() => expect(close).toHaveBeenCalledOnce());
});
it("discloses a rejected transport rather than reporting an unchanged or successful status", async () => {
  render(<BlockedDrop proposal={proposal} revision={4} disabled={false} save={async () => { throw Error("Synthetic offline transport"); }} close={() => {}}/>);
  fireEvent.change(screen.getByLabelText("Blocking reason"), { target: { value: "Keep this" } }); fireEvent.submit(screen.getByRole("form"));
  expect((await screen.findByRole("alert")).textContent).toContain("Synthetic offline transport");
  expect(screen.getByText(/status change was not confirmed/)).toBeTruthy();
});
