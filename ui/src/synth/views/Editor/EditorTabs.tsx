import type { Buffer } from "../../../Editor";
import { Led } from "../../primitives/Led";
import type { Changes } from "./changes";

/** §11.2: top-rounded keys, a cyan LED on the active one, a yellow dot when unsaved. */
export default function EditorTabs({ buffers, active, changes, onSelect, onClose }: {
  buffers: Buffer[];
  active: string | null;
  changes: Changes;
  onSelect: (path: string) => void;
  onClose: (path: string) => void;
}) {
  const current = buffers.find(buffer => buffer.path === active) ?? null;
  return <div className="synth-editor__tabstrip">
    <div className="synth-editor__tabs" role="tablist" aria-label="Open files">
      {buffers.map(buffer => {
        const dirty = buffer.text !== buffer.saved;
        return <div className="synth-editor__tab" key={buffer.path} data-active={buffer.path === active || undefined}>
          <button
            type="button"
            role="tab"
            aria-selected={buffer.path === active}
            className="synth-editor__tab-key"
            onClick={() => onSelect(buffer.path)}
          >
            <Led state={buffer.path === active ? "on" : "off"} size={6} signal="diff" />
            <span>{buffer.path.slice(buffer.path.lastIndexOf("/") + 1)}</span>
            {dirty && <span className="synth-editor__dirty" aria-label="unsaved" />}
          </button>
          <button
            type="button"
            className="synth-editor__tab-close"
            aria-label={`close ${buffer.path}`}
            onClick={() => onClose(buffer.path)}
          >×</button>
        </div>;
      })}
    </div>

    {current !== null && <div className="synth-editor__breadcrumb synth-mono">
      <span className="synth-editor__crumb-path">{current.path}</span>
      {changes.untracked
        ? <span className="synth-editor__crumb-new">new file</span>
        : changes.coarse
          ? <span className="synth-muted">too different to mark</span>
          : <>
              {changes.removedCount > 0 && <span className="synth-editor__crumb-removed">▼ {changes.removedCount} removed</span>}
              {changes.added.size > 0 && <span className="synth-editor__crumb-added">+{changes.added.size} since HEAD</span>}
            </>}
    </div>}
  </div>;
}
