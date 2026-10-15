import type { Buffer } from "../../../Editor";
import { languageName } from "../../../codemirror";
import { Led } from "../../primitives/Led";
import type { Presence } from "./presence";

const EOL = { lf: "LF", crlf: "CRLF", mixed: "Mixed", none: "None" } as const;

/**
 * The indentation this buffer actually uses.
 *
 * Measured from the text rather than from a setting, and reported as unknown when the
 * file has no indented line - a status bar that says "4 spaces" about a file with no
 * indentation is stating a preference, not a fact.
 */
export function indentOf(text: string): string {
  let tabs = 0;
  const widths = new Map<number, number>();
  for (const line of text.split("\n").slice(0, 2000)) {
    if (line.trim() === "") continue;
    if (line.startsWith("\t")) { tabs += 1; continue; }
    const spaces = line.length - line.trimStart().length;
    if (spaces > 0) widths.set(spaces, (widths.get(spaces) ?? 0) + 1);
  }
  const spaced = [...widths.values()].reduce((sum, count) => sum + count, 0);
  if (tabs === 0 && spaced === 0) return "Unknown";
  if (tabs >= spaced) return "Tabs";
  // The smallest indent that most indented lines are a multiple of: a file indented by
  // two shows plenty of fours, and the greatest common divisor is what names the unit.
  const unit = [...widths.keys()].reduce((a, b) => {
    let [x, y] = [a, b];
    while (y > 0) [x, y] = [y, x % y];
    return x;
  });
  return `${unit} space${unit === 1 ? "" : "s"}`;
}

export default function StatusBar({ buffer, branch, cursor, presence, elapsed }: {
  buffer: Buffer | null;
  branch: string | null;
  cursor: { line: number; column: number } | null;
  presence: Presence;
  elapsed: number | null;
}) {
  return <footer className="synth-editor__status synth-mono" aria-label="Editor status">
    <span className="synth-editor__status-branch">{branch ?? "detached"}</span>
    {buffer !== null && <>
      <span>{languageName(buffer.path) ?? "Plain text"}</span>
      <span>{buffer.encoding.toUpperCase()}</span>
      <span>{EOL[buffer.eol]}</span>
      <span>{indentOf(buffer.text)}</span>
      <span className="synth-editor__status-caret">
        {cursor === null ? "Ln -, Col -" : `Ln ${cursor.line}, Col ${cursor.column}`}
      </span>
    </>}
    <span className="synth-editor__status-gap" />
    {presence.working
      ? <span className="synth-editor__status-agent">
          <Led state="pulse" signal="result" size={7} />
          {presence.role ?? "agent"} working{elapsed === null ? "" : ` · ${elapsed} s`}
        </span>
      : <span className="synth-muted">No agent running here</span>}
  </footer>;
}
