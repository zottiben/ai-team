import { useRef } from "react";
import { act, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useReadoutSnapshot } from "./useReadoutSnapshot";

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
function Readout({ value, scope = "one", active = true }: { value: number; scope?: string; active?: boolean }) {
  const host = useRef<HTMLDivElement>(null), displayed = useReadoutSnapshot(host, value, scope, active);
  return <div ref={host} data-testid="readout">{displayed}</div>;
}
const shown = () => screen.getByTestId("readout").textContent;
it("coalesces bursts in the shared clock without dropping the latest value or leaving an idle timer", () => {
  vi.useFakeTimers(); const view = render(<Readout value={0} />);
  view.rerender(<Readout value={1} />); act(() => vi.advanceTimersByTime(400));
  view.rerender(<Readout value={2} />); act(() => vi.advanceTimersByTime(400));
  view.rerender(<Readout value={3} />); expect(shown()).toBe("0");
  act(() => vi.advanceTimersByTime(200)); expect(shown()).toBe("3");
  expect(vi.getTimerCount()).toBe(0); view.unmount();
});
it("never paints the previous scope and bypasses throttling in the legacy presentation", () => {
  vi.useFakeTimers(); const view = render(<Readout value={1} />);
  view.rerender(<Readout value={2} scope="two" />); expect(shown()).toBe("2");
  view.rerender(<Readout value={3} scope="two" active={false} />); expect(shown()).toBe("3");
  view.rerender(<Readout value={4} scope="two" active={false} />); expect(shown()).toBe("4");
  view.rerender(<Readout value={5} scope="two" />); expect(shown()).toBe("5");
  view.unmount(); expect(vi.getTimerCount()).toBe(0);
});
it("pauses hidden presentation and catches up on reveal rather than replaying old frames", () => {
  vi.useFakeTimers(); let hidden = false;
  vi.spyOn(document, "hidden", "get").mockImplementation(() => hidden);
  const view = render(<Readout value={1} />);
  act(() => { hidden = true; document.dispatchEvent(new Event("visibilitychange")); });
  view.rerender(<Readout value={2} />); act(() => vi.advanceTimersByTime(3000));
  expect(shown()).toBe("1"); expect(vi.getTimerCount()).toBe(0);
  act(() => { hidden = false; document.dispatchEvent(new Event("visibilitychange")); });
  expect(shown()).toBe("2"); view.unmount(); expect(vi.getTimerCount()).toBe(0);
});
