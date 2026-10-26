import { render } from "@testing-library/react";
import { expect, it } from "vitest";
import { highlight } from "./highlight";
import { SyntaxText } from "./SyntaxText";
import { syntaxInput } from "./useDiffSyntax";
import { CHUNK_ROWS, CONTEXT_ROWS, MAX_CHARS, MAX_LINE_SPANS, MAX_SPANS, type SyntaxInput, type SyntaxLine } from "./types";
import type { DiffRow } from "../DiffView";
const line = (key: string, text: string, kind: SyntaxLine["kind"] = "added", display = true): SyntaxLine => ({ key, text, kind, display });
it("colours old and new hunk excerpts independently and resets at gaps", async () => {
  const result = await highlight({ path: "example.ts", lines: [line("h0l0", "/* removed opener", "removed"), line("h0l1", "const visible = 1;"),
    line("h0l2", "const shared = 2;", "context"), line("h0l3", "/*", "context"), line("h1", "@@ gap @@", "header"), line("h1l0", "const fresh = 3;")] });
  expect(result.state).toBe("ready");
  expect(result.lines.h0l0?.some(span => span.role === "comment")).toBe(true);
  for (const key of ["h0l1", "h0l2", "h1l0"]) {
    expect(result.lines[key]?.some(span => span.role === "keyword")).toBe(true);
    expect(result.lines[key]?.some(span => span.role === "comment")).toBe(false);
  }
});
it("uses bounded preceding context without returning its decorations", async () => {
  const result = await highlight({ path: "example.ts", lines: [line("h0l0", "/*", "context", false), line("h0l1", "continued comment"), line("h0l2", "*/ const value = 1;")] });
  expect(result.lines.h0l0).toBeUndefined();
  expect(result.lines.h0l1?.every(span => span.role === "comment")).toBe(true);
  expect(result.lines.h0l2?.some(span => span.role === "keyword")).toBe(true);
});
it.each([
  ["sample.rs", "fn main() { let number = 1; }"], ["sample.json", '{"number": 1}'],
  ["sample.php", "<?php echo 'hello'; ?>"], ["sample.yaml", "number: 1"], ["Dockerfile", "FROM alpine"], ["sample.md", "# Heading"],
])("uses the existing lazy grammar for %s", async (path, text) => {
  const result = await highlight({ path, lines: [line("h0l0", text)] });
  expect(result.state).toBe("ready"); expect(result.lines.h0l0?.length).toBeGreaterThan(0);
});
it("preserves spaces, Unicode, HTML-looking source and exact original text", async () => {
  const text = '\tconst message = "😀 <script>alert(1)</script> café";  ';
  const result = await highlight({ path: "example.ts", lines: [line("h0l0", text)] });
  const view = render(<code><SyntaxText text={text} spans={result.lines.h0l0}/></code>);
  expect(view.container.textContent).toBe(text); expect(view.container.querySelector("script")).toBeNull();
  expect(view.container.querySelector(".diff-syntax-string")).toBeTruthy();
});
it("bounds input, output and unknown languages without replacing source", async () => {
  const many: SyntaxInput = { path: "example.ts", lines: Array.from({ length: CHUNK_ROWS + CONTEXT_ROWS + 1 }, (_, n) => line(`h0l${n}`, "const x = 1;")) };
  expect((await highlight(many)).state).toBe("limited");
  expect((await highlight({ path: "example.ts", lines: [line("h0l0", "x".repeat(MAX_CHARS))] })).state).toBe("limited");
  expect((await highlight({ path: "example.unknown", lines: [line("h0l0", "const x = 1;")] })).state).toBe("unsupported");
  const dense = await highlight({ path: "example.ts", lines: [line("h0l0", "const x = 1; ".repeat(300))] });
  expect(dense.state).toBe("limited");
  expect(dense.lines.h0l0?.length).toBeLessThanOrEqual(MAX_LINE_SPANS);
  expect(Object.values(dense.lines).flat().length).toBeLessThanOrEqual(MAX_SPANS);
});
it("bounds before cloning input and chooses only the visible chunk plus its prefix", () => {
  const rows: DiffRow[] = Array.from({ length: 1000 }, (_, n) => ({ kind: "line", key: `h0l${n}`, line: { kind: "added", old: null, new: n + 1, text: "const x = 1;" } }));
  const input = syntaxInput(rows, "example.ts", 2);
  expect(input?.lines.length).toBe(CHUNK_ROWS + CONTEXT_ROWS);
  expect(input?.lines.filter(row => row.display).length).toBe(CHUNK_ROWS);
  expect(input?.lines[0]?.key).toBe("h0l64");
  rows[0] = { kind: "line", key: "h0l0", line: { kind: "added", old: null, new: 1, text: "x".repeat(MAX_CHARS) } };
  Object.defineProperty(rows, "slice", { value: () => { throw Error("must bound first"); } });
  expect(syntaxInput(rows, "example.ts", 0)).toBeNull();
  expect(syntaxInput(rows, "x".repeat(4097), 1)).toBeNull();
});
