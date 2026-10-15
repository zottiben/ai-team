import type { ReactNode } from "react";
import type { Span } from "./types";

/** Offsets decorate the original string; parsed text never replaces source or anchors. */
export function SyntaxText({ text, spans }: { text: string; spans?: readonly Span[] }) {
  if (!spans?.length) return text === "" ? " " : text;
  const children: ReactNode[] = [];
  let offset = 0;
  for (const span of spans) {
    if (span.from < offset || span.to <= span.from || span.to > text.length) continue;
    if (span.from > offset) children.push(text.slice(offset, span.from));
    children.push(<span key={`${span.from}:${span.to}`} className={`diff-syntax-${span.role}`}>{text.slice(span.from, span.to)}</span>);
    offset = span.to;
  }
  if (offset < text.length) children.push(text.slice(offset));
  return <>{children}</>;
}
