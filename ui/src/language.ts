import { autocompletion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { linter, type Diagnostic as CmDiagnostic } from "@codemirror/lint";
import { hoverTooltip } from "@codemirror/view";
import type { EditorView } from "@codemirror/view";
import type { Extension } from "@codemirror/state";

import {
  lspCompletion,
  lspDiagnostics,
  lspHover,
  type LspDiagnostic,
  type Where,
} from "./api";

/** LSP counts lines and characters from zero; CodeMirror counts document offsets. */
function offsetOf(view: EditorView, line: number, character: number): number {
  // Clamped, because a server can report a position in a document the editor has already
  // changed, and an out-of-range offset throws rather than drawing nothing.
  const lineCount = view.state.doc.lines;
  const row = view.state.doc.line(Math.min(Math.max(line + 1, 1), lineCount));
  return Math.min(row.from + character, row.to);
}

function positionOf(view: EditorView, offset: number): { line: number; character: number } {
  const row = view.state.doc.lineAt(offset);
  return { line: row.number - 1, character: offset - row.from };
}

/** LSP severities are 1 error, 2 warning, 3 information, 4 hint. */
function severityOf(value: number | null): CmDiagnostic["severity"] {
  if (value === 1) return "error";
  if (value === 2) return "warning";
  return "info";
}

function toCodeMirror(view: EditorView, found: LspDiagnostic[]): CmDiagnostic[] {
  return found.map((entry) => ({
    from: offsetOf(view, entry.range.start.line, entry.range.start.character),
    to: offsetOf(view, entry.range.end.line, entry.range.end.character),
    severity: severityOf(entry.severity),
    source: entry.source ?? undefined,
    message: entry.message,
  }));
}

/**
 * Diagnostics, hover and completion from the real language server.
 *
 * Every request carries the buffer rather than naming a file on disk: the point is to see
 * a mistake before it is saved, and a server told about the saved file would report on
 * code nobody is looking at.
 */
export function languageServer(where: Where, path: string): Extension[] {
  const body = (view: EditorView, at?: { line: number; character: number }) => ({
    ...where,
    path,
    text: view.state.doc.toString(),
    ...(at ?? {}),
  });

  return [
    linter(
      async (view) => {
        try {
          const answer = await lspDiagnostics(body(view));
          // Nothing analysed and nothing published are both "draw no markers". They are
          // different from an empty list only in what they mean, and neither means clean.
          if (!answer.analysed || answer.diagnostics === null) return [];
          return toCodeMirror(view, answer.diagnostics);
        } catch {
          // A language server that is not installed must not put a red banner over an
          // editor that otherwise works.
          return [];
        }
      },
      // Long enough that typing does not queue a request per keystroke, short enough that
      // a mistake is marked while it is still on screen.
      { delay: 600 },
    ),

    hoverTooltip(async (view, pos) => {
      try {
        const answer = await lspHover(body(view, positionOf(view, pos)));
        if (answer === null || answer.text.trim() === "") return null;
        return {
          pos,
          create: () => {
            const dom = document.createElement("div");
            dom.className = "cm-lsp-hover";
            dom.textContent = answer.text;
            return { dom };
          },
        };
      } catch {
        return null;
      }
    }),

    autocompletion({
      override: [
        async (context: CompletionContext): Promise<CompletionResult | null> => {
          const word = context.matchBefore(/[\w.]+/);
          if (word === null && !context.explicit) return null;
          // A completion source can be asked without a view when CodeMirror is computing
          // off-document; there is nothing to send in that case.
          const view = context.view;
          if (view === undefined) return null;
          try {
            const at = positionOf(view, context.pos);
            const labels = await lspCompletion(body(view, at));
            if (labels.length === 0) return null;
            return {
              from: word?.from ?? context.pos,
              options: labels.map((label) => ({ label })),
            };
          } catch {
            return null;
          }
        },
      ],
    }),
  ];
}

export { offsetOf, positionOf, severityOf, toCodeMirror };
