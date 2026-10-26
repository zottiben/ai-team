import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useRef } from "react";
import { displayed, useDisplayed } from "./owner";

afterEach(() => vi.restoreAllMocks());
function Owner() {
  const ref = useRef<HTMLDivElement>(null), available = useDisplayed(ref);
  return <section data-testid="pane"><div ref={ref} data-testid="board"/><output>{String(available)}</output></section>;
}
function geometry() {
  const box = new DOMRect(0, 4000, 240, 480);
  return vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue(Object.assign([box], { item: (index: number) => index === 0 ? box : null }));
}
describe("planning owner visibility", () => {
  it("keeps an offscreen Board's visible reason form owned, but rejects a hidden pane", async () => {
    geometry(); render(<Owner/>);
    const pane = screen.getByTestId("pane"), board = screen.getByTestId("board");
    expect(displayed(board)).toBe(true); expect(screen.getByText("true")).toBeTruthy();
    act(() => { pane.hidden = true; });
    await waitFor(() => expect(screen.getByText("false")).toBeTruthy()); expect(displayed(board)).toBe(false);
    act(() => { pane.hidden = false; }); await waitFor(() => expect(screen.getByText("true")).toBeTruthy());
    act(() => { pane.style.visibility = "hidden"; }); await waitFor(() => expect(screen.getByText("false")).toBeTruthy());
  });
  it("does not force layout in the intermediate skin before React hides retained content", async () => {
    const measured = geometry(); render(<Owner/>); measured.mockClear();
    const previous = document.documentElement.dataset.synth;
    await act(async () => { document.documentElement.dataset.synth = "false"; });
    expect(measured).not.toHaveBeenCalled();
    if (previous === undefined) delete document.documentElement.dataset.synth; else document.documentElement.dataset.synth = previous;
  });
  it("tracks actual document hiding and disconnects with its owner", () => {
    geometry(); const { unmount } = render(<Owner/>), board = screen.getByTestId("board");
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    act(() => { document.dispatchEvent(new Event("visibilitychange")); });
    expect(screen.getByText("false")).toBeTruthy(); expect(displayed(board)).toBe(false);
    hidden.mockReturnValue(false); act(() => { document.dispatchEvent(new Event("visibilitychange")); });
    expect(screen.getByText("true")).toBeTruthy(); unmount(); expect(displayed(board)).toBe(false);
  });
});
