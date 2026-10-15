import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { bracketMatching, indentOnInput, syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language";
import { lintGutter } from "@codemirror/lint";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, highlightActiveLine, keymap, lineNumbers } from "@codemirror/view";

/**
 * The grammar for a file, loaded when a file of that kind is opened.
 *
 * By extension, not by sniffing content: a file is named before it is parsed, and a
 * half-written buffer should not change colour as you type. Anything unrecognised gets no
 * grammar rather than a guessed one - wrong highlighting reads as a syntax error that is
 * not there.
 *
 * Every grammar is a dynamic import, including the four that were static before. PHP
 * alone is larger than most of this application, and a grammar in the editor chunk is
 * paid for by every file, in every language, every time the editor opens. One mechanism
 * rather than two: a static list would drift the moment somebody adds the next language.
 */
const GRAMMARS: { match: (name: string, ext: string) => boolean; load: () => Promise<Extension> }[] = [
  {
    match: (_name, ext) => ext === "rs",
    load: async () => (await import("@codemirror/lang-rust")).rust(),
  },
  {
    match: (_name, ext) => ["ts", "tsx", "js", "jsx", "mjs", "cjs"].includes(ext),
    load: async () => {
      const { javascript } = await import("@codemirror/lang-javascript");
      return javascript({ typescript: true, jsx: true });
    },
  },
  {
    match: (_name, ext) => ext === "json",
    load: async () => (await import("@codemirror/lang-json")).json(),
  },
  {
    match: (_name, ext) => ["md", "mdc", "markdown"].includes(ext),
    load: async () => (await import("@codemirror/lang-markdown")).markdown(),
  },
  {
    match: (_name, ext) => ["php", "phtml"].includes(ext),
    load: async () => (await import("@codemirror/lang-php")).php(),
  },
  {
    match: (_name, ext) => ["yaml", "yml"].includes(ext),
    load: async () => (await import("@codemirror/lang-yaml")).yaml(),
  },
  {
    // Dockerfile, Dockerfile.ci, Containerfile, and `*.dockerfile`.
    match: (name, ext) =>
      ext === "dockerfile" || /^(dockerfile|containerfile)(\..+)?$/.test(name),
    // `@codemirror/legacy-modes` is pinned exactly: from 6.5 its modes are typed
    // against a split-out `@codemirror/streamparser`, whose `StringStream` is a
    // different class from the one `@codemirror/language` still bundles. The objects
    // are the same at runtime, so the only way to take the newer package is a cast -
    // and a cast around a packaging mismatch hides the next real one.
    load: async () => {
      const [{ StreamLanguage }, { dockerFile }] = await Promise.all([
        import("@codemirror/language"),
        import("@codemirror/legacy-modes/mode/dockerfile"),
      ]);
      return StreamLanguage.define(dockerFile);
    },
  },
];

/** The human name for the status bar. Null when nothing claims the file. */
export function languageName(path: string): string | null {
  const { name, ext } = nameAndExtension(path);
  if (ext === "rs") return "Rust";
  if (["ts", "tsx"].includes(ext)) return "TypeScript";
  if (["js", "jsx", "mjs", "cjs"].includes(ext)) return "JavaScript";
  if (ext === "json") return "JSON";
  if (["md", "mdc", "markdown"].includes(ext)) return "Markdown";
  if (["php", "phtml"].includes(ext)) return "PHP";
  if (["yaml", "yml"].includes(ext)) return "YAML";
  if (ext === "dockerfile" || /^(dockerfile|containerfile)(\..+)?$/.test(name)) return "Dockerfile";
  if (ext === "toml") return "TOML";
  if (ext === "css") return "CSS";
  if (ext === "sql") return "SQL";
  if (["sh", "bash", "zsh"].includes(ext)) return "Shell";
  return null;
}

function nameAndExtension(path: string): { name: string; ext: string } {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  const dot = name.lastIndexOf(".");
  return { name, ext: dot <= 0 ? "" : name.slice(dot + 1) };
}

/** Is a grammar known for this file? Used by tests and to decide whether to load one. */
export function hasGrammar(path: string): boolean {
  const { name, ext } = nameAndExtension(path);
  return GRAMMARS.some((grammar) => grammar.match(name, ext));
}

/**
 * Load the grammar for a file, or `null` when there is none.
 *
 * The failure case is deliberate: a chunk that will not load leaves the buffer plain
 * rather than leaving the editor broken. Highlighting is not what the editor is for.
 */
export async function loadGrammar(path: string): Promise<Extension | null> {
  const { name, ext } = nameAndExtension(path);
  const grammar = GRAMMARS.find((entry) => entry.match(name, ext));
  if (grammar === undefined) return null;
  try {
    return await grammar.load();
  } catch {
    return null;
  }
}

/** Reconfigured when the grammar arrives, so the buffer is usable before it does. */
export const grammarSlot = new Compartment();

/** Reconfigured when the skin changes, so a toggle does not rebuild the document. */
export const themeSlot = new Compartment();

/** Holds the change-marker gutter, which only the synth skin draws. */
export const changesSlot = new Compartment();

/**
 * The extensions every buffer gets.
 *
 * `onChange` is how dirty state is tracked: CodeMirror owns the document, so asking it
 * afterwards whether anything changed means keeping a second copy to compare against.
 */
export function extensionsFor(
  onChange: (text: string) => void,
  language: Extension[] = [],
  theme: Extension = [],
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
    grammarSlot.of([]),
    changesSlot.of([]),
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
    themeSlot.of(theme),
  ];
}

/**
 * A fresh state for one buffer.
 *
 * `separator` pins how `doc.toString()` rejoins the lines. Left to CodeMirror it is
 * always "\n", so opening a CRLF file and saving it back would rewrite every line in the
 * file - a diff nobody asked for, in a preview that promised exact bytes.
 */
export function stateFor(
  text: string,
  onChange: (text: string) => void,
  language: Extension[] = [],
  theme: Extension = [],
  separator: string | null = null,
): EditorState {
  return EditorState.create({
    doc: text,
    extensions: [
      ...(separator === null ? [] : [EditorState.lineSeparator.of(separator)]),
      ...extensionsFor(onChange, language, theme),
    ],
  });
}
