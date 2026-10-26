import { EditorView } from "@codemirror/view";
import { useEffect, useRef, type ReactNode } from "react";

import { changesSlot } from "../../../codemirror";
import { changeMarkers, setChanges, type Changes } from "./changes";
import type { Presence } from "./presence";

/** §11.2: "Moves with a 120ms transition". */
export type Cursor = { line: number; column: number };

/**
 * The code pane: CodeMirror, its change markers, and the agent's presence tag.
 *
 * The CodeMirror view is created by the editor, not here, because the buffer and its
 * undo history outlive this skin. What this adds is reconfigured into the live view, so
 * switching skins never rebuilds the document.
 */
export default function CodeView({
  surface, path, generation, changes, presence, onCursor, onViewport, scrollTo,
}: {
  surface: ReactNode;
  path: string;
  generation: number;
  changes: Changes;
  presence: Presence;
  onCursor: (cursor: Cursor) => void;
  onViewport: (viewport: { from: number; to: number }) => void;
  /** A line the minimap asked for, or null. */
  scrollTo: number | null;
}) {
  const host = useRef<HTMLDivElement>(null);
  const report = useRef({ onCursor, onViewport });
  report.current = { onCursor, onViewport };

  const view = () => (host.current === null ? null : EditorView.findFromDOM(host.current));

  // Install the markers and the watcher into whichever view is mounted now. Keyed on the
  // buffer, because a new buffer is a new view.
  useEffect(() => {
    const instance = view();
    if (instance === null) return;
    instance.dispatch({
      effects: changesSlot.reconfigure([
        changeMarkers(),
        EditorView.updateListener.of(update => {
          if (!update.selectionSet && !update.docChanged && !update.viewportChanged) return;
          const head = update.state.selection.main.head;
          const line = update.state.doc.lineAt(head);
          report.current.onCursor({ line: line.number, column: head - line.from + 1 });
          report.current.onViewport({
            from: update.state.doc.lineAt(update.view.viewport.from).number,
            to: update.state.doc.lineAt(update.view.viewport.to).number,
          });
        }),
      ]),
    });
    // The first report, so the status bar is not blank until the first keystroke.
    const head = instance.state.selection.main.head;
    const line = instance.state.doc.lineAt(head);
    report.current.onCursor({ line: line.number, column: head - line.from + 1 });
    report.current.onViewport({
      from: instance.state.doc.lineAt(instance.viewport.from).number,
      to: instance.state.doc.lineAt(instance.viewport.to).number,
    });
  }, [path, generation]);

  useEffect(() => {
    const instance = view();
    if (instance === null) return;
    instance.dispatch({ effects: setChanges.of(changes) });
  }, [changes, path, generation]);

  useEffect(() => {
    const instance = view();
    if (instance === null || scrollTo === null) return;
    const line = Math.min(Math.max(scrollTo, 1), instance.state.doc.lines);
    instance.dispatch({
      effects: EditorView.scrollIntoView(instance.state.doc.line(line).from, { y: "start" }),
    });
  }, [scrollTo]);

  // §11.2: the tag goes where the agent's last tool said it was. No tool in Pi's
  // catalogue reports a position, so the fallback - the gutter at the top of the
  // viewport - is what is drawn. A caret at a guessed line would be a fabrication.
  const here = presence.working && presence.now?.file === path;
  const placement = presence.line === null ? "viewport-top" : "line";

  return <div className="synth-editor__code" id="synth-editor-code" ref={host}>
    {here && <div
      className="synth-editor__presence"
      data-placement={placement}
      role="status"
      style={placement === "line" ? { "--presence-line": presence.line ?? 0 } as React.CSSProperties : undefined}
    >
      <span className="synth-editor__presence-tag">
        {presence.role ?? "agent"} · {presence.now?.summary ?? "working"}
      </span>
      {placement === "viewport-top" && <span className="synth-editor__presence-note">
        no line reported
      </span>}
    </div>}
    {surface}
  </div>;
}
