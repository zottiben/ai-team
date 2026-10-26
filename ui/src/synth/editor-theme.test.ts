import { EditorView } from "@codemirror/view";
import { expect, it } from "vitest";
import { stateFor, themeSlot } from "../codemirror";
import { synthEditorTheme } from "./editor-theme";

it("lets the alternate surface override legacy fonts and restore them without replacing the editor", () => {
  const host = document.createElement("div"); document.body.append(host);
  const view = new EditorView({ state: stateFor("const n = 1;", () => {}, [], synthEditorTheme()), parent: host });
  try {
    expect(getComputedStyle(view.contentDOM).fontFamily).toBe("var(--synth-font-mono)");
    view.dispatch({ effects: themeSlot.reconfigure([]) });
    expect(getComputedStyle(view.contentDOM).fontFamily).toBe("var(--font-mono)");
    view.dispatch({ effects: themeSlot.reconfigure(synthEditorTheme()) });
    expect(getComputedStyle(view.contentDOM).fontFamily).toBe("var(--synth-font-mono)");
    expect(view.state.doc.toString()).toBe("const n = 1;");
  } finally { view.destroy(); host.remove(); }
});
