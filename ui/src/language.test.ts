import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { expect, it } from "vitest";

import { offsetOf, positionOf, severityOf, toCodeMirror } from "./language";

/** A view over a known document, so offsets can be asserted exactly. */
function view(text: string): EditorView {
  return new EditorView({ state: EditorState.create({ doc: text }) });
}

const SOURCE = "fn add() {}\nfn broken() -> i32 {\n    \"not a number\"\n}\n";

it("translates a server position to a document offset", () => {
  // LSP counts lines and characters from zero; CodeMirror counts offsets from the start
  // of the document. An off-by-one here underlines the wrong word, which reads as the
  // server being wrong.
  const v = view(SOURCE);
  expect(offsetOf(v, 0, 0)).toBe(0);
  expect(offsetOf(v, 1, 0)).toBe(12); // after "fn add() {}\n"
  expect(offsetOf(v, 2, 4)).toBe(37); // the opening quote
});

it("round-trips an offset back to a position", () => {
  const v = view(SOURCE);
  for (const [line, character] of [
    [0, 0],
    [1, 3],
    [2, 4],
  ] as const) {
    expect(positionOf(v, offsetOf(v, line, character))).toEqual({ line, character });
  }
});

it("clamps a position the document no longer has", () => {
  // A server answers about the buffer it last saw. By the time the answer arrives the
  // human may have deleted those lines, and an out-of-range offset throws rather than
  // drawing nothing.
  const v = view("one\n");
  expect(() => offsetOf(v, 99, 99)).not.toThrow();
  expect(offsetOf(v, 99, 99)).toBeLessThanOrEqual(v.state.doc.length);
  expect(offsetOf(v, -5, 0)).toBe(0);
});

it("a character past the end of a line stops at the line end", () => {
  // Otherwise the underline runs into the next line.
  const v = view("ab\ncd\n");
  const row = v.state.doc.line(1);
  expect(offsetOf(v, 0, 99)).toBe(row.to);
});

it("keeps the server's severity rather than guessing one", () => {
  expect(severityOf(1)).toBe("error");
  expect(severityOf(2)).toBe("warning");
  expect(severityOf(3)).toBe("info");
  expect(severityOf(4)).toBe("info");
  // A server that sends none is not an error: treating it as one paints a red gutter for
  // a hint.
  expect(severityOf(null)).toBe("info");
});

it("marks exactly the range the server reported", () => {
  const v = view(SOURCE);
  const [marker] = toCodeMirror(v, [
    {
      range: { start: { line: 2, character: 4 }, end: { line: 2, character: 18 } },
      severity: 1,
      source: "rust-analyzer",
      message: "expected i32, found &'static str",
    },
  ]);

  expect(marker?.severity).toBe("error");
  expect(marker?.source).toBe("rust-analyzer");
  // The string literal, and nothing either side of it.
  expect(v.state.doc.sliceString(marker?.from ?? 0, marker?.to ?? 0)).toBe('"not a number"');
});

it("an empty list of diagnostics is no markers, not an error marker", () => {
  expect(toCodeMirror(view(SOURCE), [])).toEqual([]);
});
