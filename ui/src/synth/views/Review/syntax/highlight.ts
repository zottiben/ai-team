import { language } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import type { Parser } from "@lezer/common";
import { tagHighlighter, highlightTree, tags } from "@lezer/highlight";
import { hasGrammar, loadGrammar } from "../../../../codemirror";
import { CHUNK_ROWS, CONTEXT_ROWS, MAX_CHARS, MAX_LINE_SPANS, MAX_SPANS, type SyntaxInput, type SyntaxLine, type SyntaxResult, type SyntaxRole } from "./types";

const roles: readonly SyntaxRole[] = ["keyword", "variable", "string", "number", "function", "type", "comment", "punctuation"];
const highlighter = tagHighlighter([
  { tag: [tags.keyword, tags.modifier, tags.self, tags.null], class: "keyword" },
  { tag: [tags.variableName, tags.propertyName, tags.attributeName], class: "variable" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], class: "string" },
  { tag: [tags.number, tags.bool, tags.atom, tags.constant(tags.variableName)], class: "number" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.labelName, tags.link, tags.url], class: "function" },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.tagName, tags.heading, tags.strong], class: "type" },
  { tag: [tags.comment], class: "comment" },
  { tag: [tags.punctuation, tags.separator, tags.bracket, tags.operator, tags.meta, tags.processingInstruction], class: "punctuation" },
]);
// Parser instances contain grammars, not source. Bound even this small cache.
const parsers = new Map<string, Parser>();
async function parserFor(path: string) {
  const cached = parsers.get(path);
  if (cached) { parsers.delete(path); parsers.set(path, cached); return cached; }
  const extension = await loadGrammar(path);
  const parser = extension ? EditorState.create({ extensions: [extension] }).facet(language)?.parser ?? null : null;
  if (parser) parsers.set(path, parser);
  if (parsers.size > 8) parsers.delete(parsers.keys().next().value!);
  return parser;
}
const boundary = (text: string, at: number) => !(at > 0 && at < text.length && /[\uD800-\uDBFF]/.test(text[at - 1]!) && /[\uDC00-\uDFFF]/.test(text[at]!));

/** Only the worker calls this in production. Missing hunk context is not a syntax error. */
export async function highlight(input: SyntaxInput): Promise<SyntaxResult> {
  if (input.path.length > 4096 || input.lines.length > CHUNK_ROWS + CONTEXT_ROWS || input.lines.reduce((n, line) => n + line.text.length + 1, 0) > MAX_CHARS) return { state: "limited", lines: {} };
  if (!hasGrammar(input.path)) return { state: "unsupported", lines: {} };
  const parser = await parserFor(input.path);
  if (!parser) return { state: "unavailable", lines: {} };
  const result: SyntaxResult = { state: "ready", lines: {} };
  let spans = 0;
  const paint = (lines: SyntaxLine[], side: "old" | "new") => {
    let offset = 0;
    const positions = lines.filter(line => side === "old" ? line.kind !== "added" : line.kind !== "removed").map(line => {
      const start = offset; offset += line.text.length + 1; return { line, start, end: offset - 1 };
    });
    const text = positions.map(({ line }) => line.text).join("\n");
    const tree = parser.parse(text);
    let index = 0;
    highlightTree(tree, highlighter, (from, to, className) => {
      const role = roles.find(role => className.split(" ").includes(role));
      if (!role) return;
      while (index < positions.length && positions[index]!.end <= from) index++;
      for (let n = index; n < positions.length && positions[n]!.start < to; n++) {
        const position = positions[n]!, { line } = position;
        if (!line.display || (side === "old" ? line.kind !== "removed" : line.kind === "removed")) continue;
        const a = Math.max(from, position.start) - position.start, b = Math.min(to, position.end) - position.start;
        if (a >= b || !boundary(line.text, a) || !boundary(line.text, b)) continue;
        const tokens = result.lines[line.key] ??= [];
        if (tokens.length >= MAX_LINE_SPANS || spans >= MAX_SPANS) { result.state = "limited"; continue; }
        tokens.push({ from: a, to: b, role }); spans++;
      }
    });
  };
  let segment: SyntaxLine[] = [];
  const finish = () => { if (segment.length) { paint(segment, "old"); paint(segment, "new"); segment = []; } };
  for (const line of input.lines) {
    if (line.kind === "header") finish(); else segment.push(line);
  }
  finish();
  return result;
}
