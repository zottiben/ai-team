import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { Session } from "./Terminal";

const fixture = vi.hoisted(() => ({ input: (_text: string) => {}, write: vi.fn() }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  rows = 24;
  cols = 80;
  loadAddon() {}
  open() {}
  write() {}
  dispose() {}
  onData(callback: (text: string) => void) { fixture.input = callback; }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("./api", async (original) => ({
  ...(await original<typeof import("./api")>()),
  writeTerminal: fixture.write,
  resizeTerminal: () => Promise.resolve({}),
  readTerminal: () => Promise.resolve({ text: "", cursor: 0, done: true, status: 0 }),
}));

beforeEach(() => fixture.write.mockReset().mockResolvedValue({ sent: 1 }));

it("delivers terminal input in order, including Enter, even when the first request is slow", async () => {
  let first!: () => void;
  fixture.write.mockImplementationOnce(() => new Promise<void>((resolve) => { first = resolve; }));
  render(<Session id={7} />);
  await act(async () => { fixture.input("a"); fixture.input("b"); fixture.input("\r"); });
  expect(fixture.write.mock.calls).toEqual([[7, "a"]]);
  await act(async () => first());
  await waitFor(() => expect(fixture.write.mock.calls).toEqual([[7, "a"], [7, "b"], [7, "\r"]]));
});

it("stops queued input after an uncertain write, surfaces the failure and never retries", async () => {
  let fail!: (error: Error) => void;
  fixture.write.mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { fail = reject; }));
  render(<Session id={7} />);
  await act(async () => { fixture.input("command"); fixture.input("\r"); });
  await act(async () => fail(new Error("Connection lost")));
  expect((await screen.findByRole("alert")).textContent).toContain("Connection lost");
  expect(screen.getByRole("alert").textContent).toContain("not retried");
  await act(async () => fixture.input("more"));
  expect(fixture.write.mock.calls).toEqual([[7, "command"]]);
  fireEvent.click(screen.getByRole("button", { name: "Enable input after inspection" }));
  await act(async () => fixture.input("new input"));
  expect(fixture.write.mock.calls).toEqual([[7, "command"], [7, "new input"]]);
  expect(screen.queryByRole("alert")).toBeNull();
});

it("does not send queued keystrokes to a session after leaving it", async () => {
  let first!: () => void;
  fixture.write.mockImplementationOnce(() => new Promise<void>((resolve) => { first = resolve; }));
  const view = render(<Session id={7} />);
  await act(async () => { fixture.input("a"); fixture.input("\r"); });
  view.unmount();
  await act(async () => first());
  expect(fixture.write.mock.calls).toEqual([[7, "a"]]);
});
