import { afterEach, expect, it, vi } from "vitest";
import { createSyntaxClient } from "./client";
import type { SyntaxRequest, SyntaxResponse, SyntaxResult } from "./types";
class Port {
  onmessage: ((event: MessageEvent<SyntaxResponse>) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  sent: SyntaxRequest[] = [];
  terminate = vi.fn();
  postMessage = (message: SyntaxRequest) => { this.sent.push(message); };
  reply(id: number, result: SyntaxResult = { state: "ready", lines: {} }) { this.onmessage?.(new MessageEvent("message", { data: { id, result } })); }
}
function fixture() {
  const ports: Port[] = [];
  const service = createSyntaxClient(() => { const port = new Port(); ports.push(port); return port; }, 100);
  return { service, ports };
}
const input = { path: "example.ts", lines: [] };
afterEach(() => vi.useRealTimers());
it("serializes work, rejects late identities and holds no worker after its last visible owner", () => {
  const { service, ports } = fixture(), release = service.retain(), first = vi.fn(), second = vi.fn();
  service.request(input, first); service.request(input, second);
  const port = ports[0]!; expect(port.sent).toHaveLength(1);
  port.reply(900); expect(first).not.toHaveBeenCalled();
  port.reply(port.sent[0]!.id); expect(first).toHaveBeenCalledTimes(1); expect(port.sent).toHaveLength(2);
  port.reply(port.sent[0]!.id); expect(second).not.toHaveBeenCalled();
  port.reply(port.sent[1]!.id); expect(second).toHaveBeenCalledTimes(1);
  release(); release(); expect(port.terminate).toHaveBeenCalledTimes(1);
});
it("cancels obsolete parsing rather than letting it delay a newer visible chunk", () => {
  const { service, ports } = fixture(), release = service.retain(), old = vi.fn(), current = vi.fn();
  const cancel = service.request(input, old); service.request(input, current);
  cancel(); expect(ports[0]!.terminate).toHaveBeenCalledTimes(1);
  expect(ports).toHaveLength(2);
  ports[1]!.reply(ports[1]!.sent[0]!.id);
  expect(old).not.toHaveBeenCalled(); expect(current).toHaveBeenCalledTimes(1); release();
});
it("has a finite parse deadline and queue instead of hidden idle work", () => {
  vi.useFakeTimers();
  const { service, ports } = fixture(), release = service.retain(), done = vi.fn();
  for (let n = 0; n < 100; n++) service.request(input, done);
  expect(ports[0]!.sent).toHaveLength(1);
  expect(done).toHaveBeenCalledTimes(67); // one active, at most 32 queued
  vi.advanceTimersByTime(100);
  expect(done).toHaveBeenCalledTimes(100);
  expect(done.mock.calls.every(([result]) => result.state === "unavailable")).toBe(true);
  expect(ports[0]!.terminate).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(0); release();
});
it("retires a failed module context so a later explicit retry can load again", () => {
  const { service, ports } = fixture(), release = service.retain(), done = vi.fn();
  service.request(input, done);
  ports[0]!.reply(ports[0]!.sent[0]!.id, { state: "unavailable", lines: {} });
  expect(ports[0]!.terminate).toHaveBeenCalledTimes(1);
  service.request(input, done);
  expect(ports).toHaveLength(2);
  ports[1]!.reply(ports[1]!.sent[0]!.id);
  expect(done).toHaveBeenLastCalledWith({ state: "ready", lines: {} }); release();
});
it("reports worker startup failures without dropping requests or inventing tokens", () => {
  const service = createSyntaxClient(() => { throw Error("workers blocked"); }), done = vi.fn(), release = service.retain();
  service.request(input, done);
  expect(done).toHaveBeenCalledWith({ state: "unavailable", lines: {} }); release();
});
