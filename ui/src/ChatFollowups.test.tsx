import { StrictMode } from "react";
import { act, render, renderHook, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { CancelledInstructions, FollowupReceipt, useFollowupArrivals } from "./ChatFollowups";
import type { ChatFollowup } from "./chat-api";

const receipt = (state: ChatFollowup["state"]): ChatFollowup => ({ id: 8, chat_id: 1, after_node_id: 41, body: "**Keep** my work", kind: "follow_up", state, node_id: state === "queued" || state === "cancelled" ? null : 42, created_at: "2026-01-01", delivered_at: state === "delivered" ? "2026-01-02" : null });
afterEach(() => vi.useRealTimers());

it("animates acknowledgements once, survives polling, and clears its timers", () => {
  vi.useFakeTimers();
  const initialProps: { items: ChatFollowup[] | undefined } = { items: undefined };
  const hook = renderHook(({ items }) => useFollowupArrivals(items), { initialProps, wrapper: StrictMode });
  hook.rerender({ items: [receipt("queued")] });
  hook.rerender({ items: [receipt("starting")] });
  expect(hook.result.current.size).toBe(0);
  hook.rerender({ items: [receipt("delivered")] });
  expect(hook.result.current.has(8)).toBe(true);
  act(() => vi.advanceTimersByTime(450));
  hook.rerender({ items: [receipt("delivered")] });
  act(() => vi.advanceTimersByTime(450));
  expect(hook.result.current.size).toBe(0);
  hook.rerender({ items: [receipt("delivered")] });
  expect(hook.result.current.size).toBe(0);
  hook.rerender({ items: [receipt("delivered"), { ...receipt("delivered"), id: 9 }] });
  expect(hook.result.current.has(9)).toBe(true);
  hook.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it("does not replay an arrival animation for acknowledged history", () => {
  const hook = renderHook(() => useFollowupArrivals([receipt("delivered")]));
  expect(hook.result.current.size).toBe(0);
});

it("distinguishes acknowledgement from failure, completion and missing delivery evidence", () => {
  const view = render(<FollowupReceipt item={receipt("delivered")} status="failed"/>);
  expect(screen.getByRole("status").textContent).toBe("Received by agent · Turn failed");
  expect(screen.getByRole("status").title).toMatch(/not verification/);
  view.rerender(<FollowupReceipt item={{ ...receipt("starting"), image_count: 1 }} status="done"/>);
  expect(screen.getByRole("status").textContent).toMatch(/Delivery unconfirmed/);
  expect(screen.queryByText(/Received by agent/)).toBeNull();
  expect(screen.getByText(/No automatic retry.*resizing/)).not.toBeNull();
});

it("keeps cancelled instructions in collapsed readable history, never as delivered", () => {
  const view = render(<CancelledInstructions items={[receipt("cancelled"), { ...receipt("delivered"), id: 9 }]}/>);
  expect(screen.getByText("Cancelled queued messages (1) · not delivered")).not.toBeNull();
  expect(view.container.querySelector("details")?.open).toBe(false);
  expect(view.container.querySelector("strong")?.textContent).toBe("Keep");
});
