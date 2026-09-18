import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { rust } from "@codemirror/lang-rust";
import { bracketMatching, indentOnInput, syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language";
import { lintGutter } from "@codemirror/lint";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, highlightActiveLine, keymap, lineNumbers } from "@codemirror/view";

/**
 * Which grammar to highlight a file with.
 *
 * By extension, not by sniffing content: a file is named before it is parsed, and a
 * half-written buffer should not change colour as you type. Anything unrecognised gets no
 * grammar rather than a guessed one - wrong highlighting reads as a syntax error that is
 * not there.
 */
export function languageFor(path: string): Extension[] {
  const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  if (ext === "rs") return [rust()];
  if (["ts", "tsx", "js", "jsx", "mjs", "cjs"].includes(ext)) {
    return [javascript({ typescript: ext.startsWith("ts"), jsx: ext.endsWith("x") })];
  }
  if (ext === "json") return [json()];
  if (["md", "mdc", "markdown"].includes(ext)) return [markdown()];
  return [];
}

/**
 * The extensions every buffer gets.
 *
 * `onChange` is how dirty state is tracked: CodeMirror owns the document, so asking it
 * afterwards whether anything changed means keeping a second copy to compare against.
 */
export function extensionsFor(
  path: string,
  onChange: (text: string) => void,
  language: Extension[] = [],
): Extension[] {
  return [
    lineNumbers(),
    // The markers themselves. Without this the diagnostics are computed and invisible.
    lintGutter(),
    history(),
    bracketMatching(),
    indentOnInput(),
    highlightActiveLine(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    keymap.of([...defaultKeymap, ...historyKeymap]),
    ...languageFor(path),
    ...language,
    EditorView.updateListener.of((update) => {
      if (update.docChanged) onChange(update.state.doc.toString());
    }),
    // Colours come from the token layer, so the editor repaints with the window rather
    // than staying dark when it goes light.
    EditorView.theme({
      "&": { backgroundColor: "transparent", color: "var(--text-primary-default)", height: "100%" },
      ".cm-gutters": {
        backgroundColor: "transparent",
        color: "var(--text-primary-faint)",
        border: "none",
      },
      ".cm-activeLine": { backgroundColor: "var(--surface-panel-hover)" },
      ".cm-activeLineGutter": { backgroundColor: "transparent" },
      ".cm-content": { fontFamily: "var(--font-mono)" },
      // The gutter markers and the hover card, on the token layer like everything else.
      ".cm-lintRange-error": { textDecorationColor: "var(--status-failed-default)" },
      ".cm-lintRange-warning": { textDecorationColor: "var(--status-parked-default)" },
      ".cm-lsp-hover": {
        maxWidth: "40rem",
        padding: "var(--space-2)",
        whiteSpace: "pre-wrap",
        color: "var(--text-primary-default)",
        backgroundColor: "var(--surface-overlay-default)",
        border: "1px solid var(--border-subtle-default)",
        borderRadius: "var(--radius-sm)",
        font: "var(--text-sm) var(--font-mono)",
      },
      "&.cm-focused": { outline: "none" },
    }),
  ];
}

/** A fresh state for one buffer. */
export function stateFor(
  path: string,
  text: string,
  onChange: (text: string) => void,
  language: Extension[] = [],
): EditorState {
  return EditorState.create({ doc: text, extensions: extensionsFor(path, onChange, language) });
}
