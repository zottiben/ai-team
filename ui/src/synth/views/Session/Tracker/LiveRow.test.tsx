import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import LiveRow from "./LiveRow";

const started = "2026-01-01T00:00:00Z", now = Date.parse(started);
it.each([null, "bad stamp", "2026-01-01T00:00:01Z"])("does not claim zero wall time for an unavailable or reversed start: %s", start => {
  render(<LiveRow stopping={false} started={start} now={now}/>);
  expect(screen.getByText("Wall elapsed unavailable")).toBeTruthy();
});
it("keeps a valid zero and understands stored UTC and offset stamps", () => {
  const view = render(<LiveRow stopping={false} started={started} now={now}/>);
  expect(screen.getByText("0s wall elapsed")).toBeTruthy();
  view.rerender(<LiveRow stopping={false} started="2026-01-01 00:00:00" now={now}/>);
  expect(screen.getByText("0s wall elapsed")).toBeTruthy();
  view.rerender(<LiveRow stopping={false} started="2026-01-01T01:00:00+01:00" now={now}/>);
  expect(screen.getByText("0s wall elapsed")).toBeTruthy();
  view.rerender(<LiveRow stopping={false} started={started} now={Number.NaN}/>);
  expect(screen.getByText("Wall elapsed unavailable")).toBeTruthy();
});
it("announces state changes without repeatedly announcing the one-hertz clock", () => {
  const view = render(<LiveRow stopping={false} started={started} now={now + 1000}/>);
  expect(screen.getByRole("status").textContent).toBe("Working…");
  view.rerender(<LiveRow stopping={false} started={started} now={now + 2000}/>);
  expect(screen.getByRole("status").textContent).toBe("Working…"); expect(screen.getByText("2s wall elapsed")).toBeTruthy();
  view.rerender(<LiveRow stopping started={started} now={now + 2000}/>);
  expect(screen.getByRole("status").textContent).toBe("Stopping the current turn…");
});
