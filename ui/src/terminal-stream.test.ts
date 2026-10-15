import { afterEach, expect, it, vi } from "vitest";
import { streamOutput } from "./terminal-stream";

afterEach(() => vi.useRealTimers());

it("distinguishes initial scrollback from live observations, including an empty initial read", async () => {
  vi.useFakeTimers();
  for (const initial of ["history", ""]) {
    const read = vi.fn().mockResolvedValueOnce({ text: initial, cursor: 7, done: false, status: null })
      .mockResolvedValueOnce({ text: "live", cursor: 11, done: true, status: 0 });
    const text = vi.fn();
    streamOutput(1, { text, done: vi.fn() }, read);
    await vi.advanceTimersByTimeAsync(120);
    if (initial) expect(text).toHaveBeenNthCalledWith(1, initial, true);
    expect(text).toHaveBeenLastCalledWith("live", false);
  }
});

it("does not deliver a late response to a disposed renderer", async () => {
  type Chunk = { text: string; cursor: number; done: boolean; status: number | null };
  let complete: (value: Chunk) => void = () => { throw new Error("reader not started"); };
  const text = vi.fn(), done = vi.fn();
  const stop = streamOutput(1, { text, done }, () => new Promise(resolve => { complete = resolve; }));
  stop();
  complete({ text: "late", cursor: 4, done: true, status: 0 });
  await Promise.resolve();
  expect(text).not.toHaveBeenCalled();
  expect(done).not.toHaveBeenCalled();
});
