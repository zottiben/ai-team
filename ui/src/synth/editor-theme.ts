import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

/**
 * SPEC §11.3, as CodeMirror extensions.
 *
 * Every value is a token. A hex literal here would be a buffer that keeps its midnight
 * background when the window goes light, which is exactly the bug the token rule exists
 * to prevent - and the editor is the one surface nobody notices it on until they switch.
 *
 * Loaded with the editor chunk, never at startup: this module imports CodeMirror.
 */
const code = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword, tags.operatorKeyword], color: "var(--code-keyword)" },
  { tag: [tags.definitionKeyword, tags.moduleKeyword, tags.self, tags.null], color: "var(--code-keyword)" },
  { tag: [tags.variableName, tags.propertyName, tags.attributeName], color: "var(--code-variable)" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], color: "var(--code-string)" },
  { tag: [tags.number, tags.bool, tags.atom, tags.constant(tags.variableName)], color: "var(--code-number)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.labelName], color: "var(--code-function)" },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.tagName], color: "var(--code-type)" },
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment], color: "var(--code-comment)", fontStyle: "italic" },
  { tag: [tags.punctuation, tags.separator, tags.bracket, tags.operator], color: "var(--code-punctuation)" },
  { tag: [tags.meta, tags.processingInstruction], color: "var(--code-punctuation)" },
  { tag: [tags.heading, tags.strong], color: "var(--code-type)", fontWeight: "600" },
  { tag: [tags.emphasis], color: "var(--code-variable)", fontStyle: "italic" },
  { tag: [tags.link, tags.url], color: "var(--code-function)", textDecoration: "underline" },
  { tag: [tags.invalid], color: "var(--code-removed)" },
]);

/** Mono 12.5/22, 52px gutter, 2px pearl caret: the measurements are the spec's. */
const surface = EditorView.theme(
  {
    "&": {
      backgroundColor: "var(--code-surface)",
      color: "var(--code-text)",
      height: "100%",
      fontFamily: "var(--synth-font-mono)",
      fontSize: "12.5px",
    },
    ".cm-scroller": { fontFamily: "var(--synth-font-mono)", lineHeight: "22px" },
    ".cm-content": { fontFamily: "var(--synth-font-mono)", caretColor: "var(--code-caret)" },
    ".cm-gutters": {
      backgroundColor: "var(--code-surface)",
      color: "var(--code-gutter)",
      border: "none",
      minWidth: "52px",
    },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px 0 8px", minWidth: "36px" },
    ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--code-gutter-active)" },
    ".cm-activeLine": { backgroundColor: "var(--code-active-line)" },
    "&.cm-focused .cm-cursor, .cm-cursor": {
      borderLeftWidth: "2px",
      borderLeftColor: "var(--code-caret)",
    },
    // §11.2's blink. The stylesheet stops it under reduced motion, where a 1Hz loop is
    // exactly what the setting is asking not to have.
    ".cm-cursorLayer": { animationDuration: "1.06s", animationTimingFunction: "steps(1)" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "var(--code-selection)",
    },
    ".cm-selectionMatch": { backgroundColor: "var(--code-selection)" },
    "&.cm-focused": { outline: "none" },
    ".cm-lsp-hover": {
      maxWidth: "40rem",
      padding: "8px",
      whiteSpace: "pre-wrap",
      color: "var(--code-text)",
      backgroundColor: "var(--synth-panel)",
      border: "1px solid var(--synth-hairline)",
      borderRadius: "var(--r-card)",
      font: "12px var(--synth-font-mono)",
    },
    ".cm-tooltip": {
      backgroundColor: "var(--synth-panel)",
      border: "1px solid var(--synth-hairline)",
      color: "var(--code-text)",
    },
    ".cm-tooltip-autocomplete ul li[aria-selected]": {
      backgroundColor: "var(--code-selection)",
      color: "var(--code-text)",
    },
    // The change marker column (§11.2): a 3px bar beside an added line and a tint
    // across it, a wedge under the line a removal sat below.
    ".cm-synth-change": { width: "14px", paddingLeft: "4px" },
    ".cm-synth-change-added": {
      borderLeft: "3px solid var(--code-added)",
      boxShadow: "var(--code-added-glow)",
    },
    ".cm-synth-change-removed": {
      borderBottom: "3px solid var(--code-removed)",
    },
    ".cm-line.cm-synth-added": { backgroundColor: "var(--code-added-tint)" },
  },
  // Dark is a lie here: the palette is whichever theme the window is in, and saying
  // `dark: true` would make CodeMirror pick its own light-on-dark defaults for anything
  // the theme above does not cover.
  { dark: false },
);

/** The whole §11.3 mapping, for `stateFor`'s theme slot. */
export function synthEditorTheme(): Extension {
  return [surface, syntaxHighlighting(code)];
}
