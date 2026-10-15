import { Fragment, useEffect, useRef, useState } from "react";
import { useSynth } from "../../../ui-preferences";
import { FileView } from "../../../ReviewFile";
import { committedFile, committedTree, type Changes, type CommitEntry, type CommitFile, type DeliveryAction } from "../../../changes-api";
import type { useReviewFix } from "../../../useReviewFix";
import { currentReview, deliveryActions, type ReviewSession } from "./drafts";

type Props = {
  chatId: number; session: ReviewSession; active: boolean; changes: Changes; mode: "single" | "team";
  unavailable: boolean; busy: boolean; disabled: boolean;
  command: (work: () => Promise<void>) => Promise<boolean>; submitFix: ReturnType<typeof useReviewFix>;
  onDirty: (dirty: boolean) => void; onPreview: (action: DeliveryAction) => void;
};
/** One retained owner per exact draft and checkout generation, never another dispatcher. */
export function DraftReviewPanel({ chatId, session, active, changes, mode, unavailable, busy, disabled, command, submitFix, onDirty, onPreview }: Props) {
  const { review, epoch, workspace } = session, selected = review.draft;
  const fresh = currentReview(session, changes), synth = useSynth();
  const [expanded, setExpanded] = useState<number>();
  const [finding, setFinding] = useState(""), [attempted, setAttempted] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState(false), [drafts, setDrafts] = useState<ReadonlySet<string>>(new Set());
  const [tree, setTree] = useState<CommitEntry[] | null>(null), [path, setPath] = useState(""), [file, setFile] = useState<CommitFile | null>(null);
  const dirty = finding.length > 0 || attempted !== null || drafts.size > 0;
  const report = useRef(onDirty); report.current = onDirty;
  useEffect(() => report.current(dirty), [dirty]);
  const retry = attempted !== null && finding === attempted;
  const canSend = !busy && !!finding.trim() && (retry || (!unavailable && fresh && mode === "team"));
  return <section hidden={!active} aria-label="Draft review">
    <h4>Review {selected.target.slice_key} · {selected.commit_sha.slice(0, 12)}</h4>
    <p className="mono">{selected.base_sha} → {selected.commit_sha}</p>
    <p className="mono">Captured {workspace} · generation {epoch} · draft revision {selected.target.revision}</p>
    <p className="faint">Submitting a comment sends it to the maker for repair and independent checks. Only this slice is approved; no push, PR or merge.</p>
    {mode !== "team" && <p className="notice">Switch to team to repair this verified slice. The captured notes remain here; no mode change is automatic.</p>}
    {submitted && <p role="status">Review submitted to the agent. Follow its recorded repair attempt in this chat.</p>}
    {review.files.map((file, index) => <Fragment key={file.path}>
      {synth && index === 3 && <div className="chat-controls"><span>{review.files.length - 3} more captured files</span>
        <button className="button" type="button" onClick={() => setExpanded(value => (value ?? 0) + 1)}>Expand all readable files</button></div>}
      <FileView key={file.path} file={file} commentTarget="agent" retryCapturedRequest revision={selected.commit_sha} initiallyCollapsed={synth && index >= 3} expand={expanded}
      commentScope={session.key} reviewKey={`chat:${chatId}:draft:${session.key}`} onState={state => setDrafts(previous => {
        if (previous.has(file.path) === state.draft) return previous;
        const next = new Set(previous); if (state.draft) next.add(file.path); else next.delete(file.path); return next;
      })} onComment={unavailable || !fresh || mode !== "team" ? undefined : async (body, anchor) => command(async () => {
        await submitFix({ kind: "draft", target: selected.target, body, anchor: { path: anchor.file_path, side: anchor.side, line: anchor.line_start } }, epoch, session.key);
        setSubmitted(true);
      })}/></Fragment>)}
    <button className="button" disabled={busy} onClick={() => void command(async () => { setTree(await committedTree(chatId, selected.target)); })}>Browse committed tree</button>
    {tree && <div>
      <label>File in reviewed commit<select value={path} disabled={busy} onChange={event => { if (!busy) { setPath(event.target.value); setFile(null); } }}>
        <option value="">Choose a committed file</option>
        {tree.map(entry => <option key={entry.path} value={entry.path}>{entry.path} · {entry.kind} · {entry.size ?? "?"} bytes</option>)}
      </select></label>
      <button className="button" disabled={busy || !path} onClick={() => void command(async () => { setFile(await committedFile(chatId, selected.target, path)); })}>Read committed file</button>
      {file && <div aria-label="Committed file"><p className="mono">{file.path} · {file.commit_sha}</p>{file.reason ? <p>{file.reason}</p> : <pre className="chat-commit-content">{file.text}</pre>}</div>}
    </div>}
    {review.findings.map(event => <p key={event.id} className="notice">{event.summary}</p>)}
    <form onSubmit={event => {
      event.preventDefault(); if (!canSend) return;
      const body = finding; setAttempted(body);
      void command(async () => {
        await submitFix({ kind: "draft", target: selected.target, body, anchor: null }, epoch, session.key);
        setFinding(current => current === body ? "" : current); setAttempted(null); setSubmitted(true);
      });
    }}>
      <label>Review finding<textarea value={finding} onChange={event => setFinding(event.target.value)} rows={3} maxLength={32000}/></label>
      <p className="faint">Send this finding for repair of this exact slice, without approving publication.</p>
      {retry && <p className="notice">Retry uses the original request, draft and checkout generation; it does not submit against a replacement review.</p>}
      {attempted !== null && !retry && <button type="button" className="button" disabled={busy} onClick={() => setFinding(attempted)}>Restore attempted finding for retry</button>}
      <button className="button" disabled={!canSend}>Send finding for repair</button>
      {(finding.length > 0 || attempted !== null) && <button type="button" className="button" disabled={busy} onClick={() => { setFinding(""); setAttempted(null); }}>Discard local finding</button>}
      {attempted !== null && <p className="faint">Discarding local text does not cancel a repair that may already have started.</p>}
    </form>
    {!fresh && <p className="notice">This draft changed, or its captured checkout generation is no longer current. Review it again before acting. Unsent notes remain bound to the original snapshot.</p>}
    {disabled && <p className="faint">Finish or close the current execution before delivery.</p>}
    <div className="chat-controls">{(Object.keys(deliveryActions) as DeliveryAction[]).map(action => <button key={action} className="button" disabled={unavailable || !fresh} onClick={() => onPreview(action)}>Preview: {deliveryActions[action]}</button>)}</div>
  </section>;
}
