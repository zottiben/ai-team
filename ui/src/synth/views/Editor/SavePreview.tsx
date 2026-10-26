import { useEffect } from "react";
import type { Pending } from "../../../Editor";
import type { Refusal } from "../../../editor-api";
import { HwButton } from "../../primitives/HwButton";
import { Lcd } from "../../primitives/Lcd";
import { Led } from "../../primitives/Led";

const short = (hash: string) => hash.slice(0, 12);

/**
 * §11.4 step one: "a diff preview of exactly what will be written".
 *
 * Exactly, and said in full: the checkout, the path, the fingerprint the file was opened
 * at, the byte count and hash of the proposal, and the hunks the server computed from
 * the bytes on disk. The confirm button writes those bytes and nothing else - the server
 * refuses a body that does not hash to the preview shown here.
 */
export default function SavePreview({ pending, busy, onConfirm, onCancel }: {
  pending: Pending; busy: boolean; onConfirm: () => void; onCancel: () => void;
}) {
  const { preview } = pending;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") onCancel(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return <div className="synth-save" role="dialog" aria-modal="true" aria-label={`Save ${preview.path}`}>
    <div className="synth-save__sheet">
      <header className="synth-save__head">
        <span className="synth-silk">Write to disk</span>
        <span className="synth-mono synth-save__path">{preview.path}</span>
      </header>

      <Lcd className="synth-save__facts" variant={preview.stale ? "alert" : undefined}>
        <div><span className="synth-lcd__label">Checkout</span><span className="synth-mono">{preview.checkout}</span></div>
        <div><span className="synth-lcd__label">Opened at</span><span className="synth-mono">{short(preview.base.hash)} · {preview.base.bytes} bytes</span></div>
        <div><span className="synth-lcd__label">Will write</span><span className="synth-mono">{short(preview.proposed.hash)} · {preview.proposed.bytes} bytes · {preview.proposed.eol.toUpperCase()}</span></div>
        <div><span className="synth-lcd__label">Change</span><span className="synth-mono">+{preview.change.added} / -{preview.change.removed}</span></div>
      </Lcd>

      {preview.stale && <p className="synth-save__refusal" role="alert">
        <Led state="pulse" signal="attention" size={8} />
        Changed on disk since you opened it - reload or compare
      </p>}
      {preview.notices.map(notice => <p className="synth-muted" key={notice}>{notice}</p>)}
      {preview.change.coarse && <p className="synth-muted">
        Too far apart to compare line by line; the whole file is replaced.
      </p>}

      {preview.change.hunks.length === 0
        ? <p className="synth-muted">No change: these bytes are already on disk.</p>
        : <div className="synth-save__diff">
            {preview.change.hunks.map(hunk => <div className="synth-save__hunk" key={`${hunk.before_start}-${hunk.after_start}`}>
              <div className="synth-save__hunk-head synth-mono">@@ -{hunk.before_start},{hunk.before_lines} +{hunk.after_start},{hunk.after_lines} @@</div>
              {hunk.lines.map((line, index) => <div className="synth-save__line synth-mono" data-kind={line.kind} key={index}>
                <span className="synth-save__num">{line.before ?? ""}</span>
                <span className="synth-save__num">{line.after ?? ""}</span>
                <span className="synth-save__sign" aria-hidden="true">{line.kind === "added" ? "+" : line.kind === "removed" ? "-" : " "}</span>
                <span className="synth-save__text">{line.text === "" ? " " : line.text}</span>
              </div>)}
            </div>)}
            {preview.change.truncated && <p className="synth-muted">More hunks than shown; the write still covers the whole file.</p>}
          </div>}

      <footer className="synth-save__actions">
        <HwButton autoFocus variant="primary" disabled={busy || preview.stale} onClick={onConfirm}>
          Write these bytes
        </HwButton>
        <HwButton onClick={onCancel}>Cancel</HwButton>
      </footer>
    </div>
  </div>;
}

/** §11.4 step two, when the answer is no. Both versions survive it. */
export function SaveRefusal({ refusal, onReload, onDismiss }: {
  refusal: Refusal; onReload: () => void; onDismiss: () => void;
}) {
  return <div className="synth-save" role="alertdialog" aria-modal="true" aria-label="Save refused">
    <div className="synth-save__sheet">
      <header className="synth-save__head">
        <Led state="pulse" signal="attention" size={10} />
        <span className="synth-silk">Not written</span>
      </header>
      <p className="synth-save__refusal">{refusal.message}</p>
      <p className="synth-muted">
        Your buffer is untouched and so is the file. Reload to take the version on disk -
        that discards your edits - or keep yours and compare.
      </p>
      {(refusal.preserved ?? []).map(kept => <p className="synth-muted" key={kept.name}>
        Kept, not deleted: <span className="synth-mono">{kept.path}</span> ({kept.bytes} bytes).
      </p>)}
      {refusal.disk !== null && <Lcd className="synth-save__facts">
        <div><span className="synth-lcd__label">On disk</span><span className="synth-mono">{short(refusal.disk.hash)} · {refusal.disk.bytes} bytes</span></div>
      </Lcd>}
      {refusal.disk_text !== null && <details className="synth-save__disk">
        <summary>What is on disk now</summary>
        <pre className="synth-mono">{refusal.disk_text.text}</pre>
        {refusal.disk_text.truncated && <p className="synth-muted">Shown in part.</p>}
      </details>}
      <footer className="synth-save__actions">
        <HwButton onClick={onReload}>Reload from disk</HwButton>
        <HwButton variant="primary" onClick={onDismiss}>Keep my version</HwButton>
      </footer>
    </div>
  </div>;
}
