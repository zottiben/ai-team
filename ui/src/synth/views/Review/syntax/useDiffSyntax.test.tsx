import { useRef } from "react";
import { act, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useDiffSyntax } from "./useDiffSyntax";
import type { DiffRow } from "../DiffView";
import type { SyntaxInput, SyntaxResult } from "./types";
const service = vi.hoisted(() => ({ retain: vi.fn<() => () => void>(), request: vi.fn<(input: SyntaxInput, done: (result: SyntaxResult) => void) => () => void>() }));
vi.mock("./client", () => ({ syntaxClient: service }));
const rows: DiffRow[] = [{ key: "h0l0", kind: "line", line: { kind: "added", old: null, new: 1, text: "const value = 1;" } }];
const entries = [{ index: 0 }];
function Harness({ source, enabled = true, content = rows, shown = entries }: { source: string; enabled?: boolean; content?: DiffRow[]; shown?: { index: number }[] }) {
  const host = useRef<HTMLElement>(null), value = useDiffSyntax(content, "example.ts", source, shown, enabled, host);
  return <section ref={host}><p data-testid="count">{value.chunks?.size ?? 0}</p><p data-testid="state">{value.state}</p><p data-testid="token">{value.chunks?.get(0)?.lines.h0l0?.[0]?.role ?? "plain"}</p></section>;
}
beforeEach(() => { service.retain.mockReset().mockImplementation(() => vi.fn()); service.request.mockReset().mockImplementation(() => vi.fn()); });
it("does not accept another source generation's delayed decorations", () => {
  const view = render(<Harness source="first"/>), old = service.request.mock.calls[0]?.[1];
  expect(old).toBeDefined();
  view.rerender(<Harness source="second"/>);
  act(() => old?.({ state: "ready", lines: { h0l0: [{ from: 0, to: 5, role: "comment" }] } }));
  expect(screen.getByTestId("token").textContent).toBe("plain");
  act(() => service.request.mock.calls[1]?.[1]({ state: "ready", lines: { h0l0: [{ from: 0, to: 5, role: "keyword" }] } }));
  expect(screen.getByTestId("token").textContent).toBe("keyword");
  expect(service.request).toHaveBeenCalledTimes(2);
});
it("does not evict a still-requested pinned chunk or grow its cache while scrolling", () => {
  const content: DiffRow[] = Array.from({ length: 1400 }, (_, n) => ({ key: `h0l${n}`, kind: "line", line: { kind: "added", old: null, new: n + 1, text: "const value = 1;" } }));
  const view = render(<Harness source="same" content={content}/>);
  act(() => service.request.mock.calls.at(-1)?.[1]({ state: "ready", lines: {} }));
  for (let n = 1; n <= 20; n++) {
    view.rerender(<Harness source="same" content={content} shown={[{ index: 0 }, { index: n * 64 }]}/>);
    act(() => service.request.mock.calls.at(-1)?.[1]({ state: "ready", lines: {} }));
    expect(screen.getByTestId("state").textContent).toBe("ready");
    expect(Number(screen.getByTestId("count").textContent)).toBeLessThanOrEqual(16);
  }
  expect(service.request).toHaveBeenCalledTimes(21);
});
it("has no legacy worker and cancels when the skin stops using syntax", () => {
  const release = vi.fn(), cancel = vi.fn(); service.retain.mockReturnValue(release); service.request.mockReturnValue(cancel);
  const view = render(<Harness source="first" enabled={false}/>);
  expect(service.request).not.toHaveBeenCalled(); expect(service.retain).not.toHaveBeenCalled();
  view.rerender(<Harness source="first"/>); expect(service.request).toHaveBeenCalledTimes(1);
  view.rerender(<Harness source="first" enabled={false}/>);
  expect(release).toHaveBeenCalledTimes(1); expect(cancel).toHaveBeenCalledTimes(1);
  act(() => service.request.mock.calls[0]?.[1]({ state: "ready", lines: { h0l0: [{ from: 0, to: 5, role: "keyword" }] } }));
  expect(screen.getByTestId("token").textContent).toBe("plain");
});
