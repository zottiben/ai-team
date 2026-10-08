import { lazy, Suspense, useEffect, useState } from "react";
const ChatCheckout = lazy(() => import("./ChatCheckout").then(m => ({ default: m.ChatCheckout })));
import { FileView } from "./Review";
import { useReviewFix } from "./useReviewFix";
import {
  acknowledgeDelivery, approveDelivery, chatChanges, committedFile, committedTree, inspectDelivery, previewDelivery, reviewDraft,
  type Changes, type CommitEntry, type CommitFile, type Delivery, type DeliveryAction, type DeliveryInspection, type DraftReview,
} from "./changes-api";
import "./chat-changes.css";

const ACTIONS: Record<DeliveryAction, string> = {
  integrate: "Integrate locally", push: "Push draft", pull_request: "Open draft PR",
};
export function ChatChanges({ chatId, tick, disabled, onChanged, onFeedback }: {
  chatId: number; tick: number; disabled: boolean; onChanged: () => void; onFeedback?: (text: string) => void;
}) {
  const submitFix = useReviewFix(chatId);
  const [submitted, setSubmitted] = useState(false);
  const [changes, setChanges] = useState<Changes | null>(null);
  const [review, setReview] = useState<DraftReview | null>(null);
  const [preview, setPreview] = useState<Delivery | null>(null);
  const [finding, setFinding] = useState("");
  const [tree, setTree] = useState<CommitEntry[] | null>(null);
  const [path, setPath] = useState("");
  const [file, setFile] = useState<CommitFile | null>(null);
  const [inspection, setInspection] = useState<DeliveryInspection | null>(null);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const changeTick = disabled ? 0 : tick;
  useEffect(() => {
    let current = true;
    void chatChanges(chatId).then((value) => {
      if (current) setChanges(value);
    }).catch((error: unknown) => { if (current) setProblem(String(error)); });
    return () => { current = false; };
  }, [chatId, changeTick, disabled, refresh]);
  async function command(work: () => Promise<void>) {
    setBusy(true); setProblem(null);
    try { await work(); return true; }
    catch (error: unknown) { setProblem(error instanceof Error ? error.message : String(error)); return false; }
    finally { setBusy(false); onChanged(); setRefresh((n) => n + 1); }
  }
  const pending = changes?.deliveries.some((d) => d.state === "running" || d.state === "inspection");
  const unavailable = disabled || busy || pending;
  const selected = review?.draft;
  const fresh = selected && changes?.drafts.some((d) => d.target.run_id === selected.target.run_id && d.target.slice_key === selected.target.slice_key && d.target.revision === selected.target.revision && d.commit_sha === selected.commit_sha);
  return <section className="chat-overview-card chat-changes" aria-label="Changes and delivery">
    <div className="chat-changes-heading"><h3>Changes and delivery</h3>
      <button className="button" disabled={busy} onClick={() => { setProblem(null); setRefresh((n) => n + 1); }}>Refresh changes</button></div>
    {problem && <p role="alert" className="error">{problem}</p>}
    {!changes ? <p className="faint">Reading actual Git changes…</p> : <>
      {changes.issues.length > 0 && <div className="notice"><strong>Checkout or draft inspection is incomplete</strong><ul>{changes.issues.map((issue, n) => <li key={n}>{issue}</li>)}</ul></div>}
      <details>
        <summary>Working checkout · {changes.branch ?? "detached"} · {changes.staged.length} staged · {changes.unstaged.length} unstaged · {changes.untracked.length} untracked</summary>
        <Suspense fallback={<p>Loading checkout actions…</p>}><ChatCheckout chatId={chatId} tick={tick+refresh} disabled={disabled||!!pending} onChanged={onChanged} onFeedback={onFeedback}/></Suspense>
      </details>
      <h4>Verified drafts from this chat</h4>
      <p className="faint">Verification is not integration or publication. Each action needs its own exact approval. Draft review reads commit objects, not a returned worktree.</p>
      {!changes.drafts.length && <p>No verified draft commits yet.</p>}
      {changes.drafts.map((draft) => <div className="chat-draft" key={`${draft.target.run_id}:${draft.target.slice_key}`}>
        <strong>{draft.target.slice_key} · run #{draft.target.run_id}</strong>
        <span className="mono">{draft.commit_sha}</span>
        <span className="faint">{draft.branch} · lease {draft.lease_state}</span>
        <button className="button" disabled={busy} onClick={() => void command(async () => {
          setReview(await reviewDraft(chatId, draft.target)); setPreview(null); setFinding(""); setTree(null); setPath(""); setFile(null);
        })}>Review {draft.target.slice_key}</button>
      </div>)}
      {review && selected && <section aria-label="Draft review">
        <h4>Review {selected.target.slice_key} · {selected.commit_sha.slice(0, 12)}</h4>
        <p className="mono">{selected.base_sha} → {selected.commit_sha}</p>
        <p className="faint">Submitting a comment sends it to the maker for repair and independent checks. Only this slice is approved; no push, PR or merge.</p>
        {submitted && <p role="status">Review submitted to the agent. Follow its new attempt in this chat.</p>}
        {review.files.map((file) => <FileView key={file.path} file={file} onComment={unavailable || !fresh ? undefined : async (body, anchor) => command(async () => {
          await submitFix({ kind: "draft", target: selected.target, body, anchor: { path: anchor.file_path, side: anchor.side, line: anchor.line_start } }, changes.workspace_epoch);
          setSubmitted(true);
        })} />)}
        <button className="button" disabled={busy} onClick={() => void command(async () => { setTree(await committedTree(chatId, selected.target)); })}>Browse committed tree</button>
        {tree && <div>
          <label>File in reviewed commit<select value={path} onChange={(event) => { setPath(event.target.value); setFile(null); }}>
            <option value="">Choose a committed file</option>
            {tree.map((entry) => <option key={entry.path} value={entry.path}>{entry.path} · {entry.kind} · {entry.size ?? "?"} bytes</option>)}
          </select></label>
          <button className="button" disabled={busy || !path} onClick={() => void command(async () => { setFile(await committedFile(chatId, selected.target, path)); })}>Read committed file</button>
          {file && <div aria-label="Committed file"><p className="mono">{file.path} · {file.commit_sha}</p>{file.reason ? <p>{file.reason}</p> : <pre className="chat-commit-content">{file.text}</pre>}</div>}
        </div>}
        {review.findings.map((event) => <p key={event.id} className="notice">{event.summary}</p>)}
        <form onSubmit={(event) => { event.preventDefault(); void command(async () => {
          await submitFix({ kind: "draft", target: selected.target, body: finding, anchor: null }, changes.workspace_epoch); setFinding(""); setSubmitted(true);
        }); }}>
          <label>Review finding<textarea value={finding} onChange={(e) => setFinding(e.target.value)} rows={3} maxLength={32000} /></label>
          <p className="faint">Send this finding for repair of this exact slice, without approving publication.</p>
          <button className="button" disabled={unavailable || !fresh || !finding.trim()}>Send finding for repair</button>
        </form>
        {!fresh && <p className="notice">This draft changed. Review it again before acting.</p>}
        {disabled && <p className="faint">Finish or close the current execution before delivery.</p>}
        <div className="chat-controls">{(Object.keys(ACTIONS) as DeliveryAction[]).map((action) => <button key={action} className="button" disabled={unavailable || !fresh}
          onClick={() => void command(async () => { setPreview(await previewDelivery(chatId, selected.target, action)); })}>Preview: {ACTIONS[action]}</button>)}</div>
      </section>}
      {preview && <section className="notice" aria-label="Delivery approval">
        <h4>{ACTIONS[preview.snapshot.action]} — exact approval</h4>
        <dl>
          <dt>Draft</dt><dd className="mono">{preview.snapshot.commit_sha}</dd>
          <dt>Checkout</dt><dd className="mono">{preview.snapshot.workspace_path} · {preview.snapshot.checkout_branch} · {preview.snapshot.checkout_head}</dd>
          {preview.snapshot.remote_url && <><dt>Destination</dt><dd className="mono">{preview.snapshot.remote_url} · {preview.snapshot.delivery_branch}</dd></>}
          {preview.snapshot.base_branch && <><dt>PR base</dt><dd className="mono">{preview.snapshot.github_repo} · {preview.snapshot.base_branch} · {preview.snapshot.base_sha}</dd></>}
        </dl>
        <p>{preview.snapshot.action === "integrate"
          ? "Fast-forward this clean checkout only. Dirty files or divergent history cause refusal; nothing is stashed, reset or cherry-picked."
          : preview.snapshot.action === "push" ? "Publish only this commit to the named ref. Do not open or merge a PR."
            : "Create an unmerged draft PR for the already published commit. Do not retarget an existing PR or request a merge."}</p>
        <button className="button button--primary" disabled={unavailable || !fresh} onClick={() => void command(async () => {
          const result = await approveDelivery(chatId, preview); setPreview(null);
          if (result.state !== "done") throw new Error(result.result ?? "Delivery did not finish; inspect its recorded state.");
        })}>Approve: {ACTIONS[preview.snapshot.action]}</button>
        <button className="button" disabled={busy} onClick={() => setPreview(null)}>Dismiss delivery</button>
      </section>}
      {changes.deliveries.some((d) => d.state !== "preview") && <section aria-label="Delivery evidence"><h4>Recent delivery evidence</h4><p className="faint">Up to 100 outcomes; unresolved deliveries stay first. Full history remains in this chat's execution evidence.</p>
        {changes.deliveries.filter((d) => d.state !== "preview").map((d) => <div className="notice" key={d.id}>
          <strong>{ACTIONS[d.snapshot.action]} · {d.state}</strong><p className="mono">{d.snapshot.commit_sha}</p>
          <p>{d.result ?? "The approved command is running or was interrupted. No automatic retry or cleanup; inspect its outcome."}</p>
          {d.state === "done" && d.snapshot.action === "pull_request" && d.result?.startsWith("https://") && <a href={d.result} target="_blank" rel="noreferrer">Open pull request</a>}
          {(d.state === "running" || d.state === "inspection") && <button className="button" disabled={busy} onClick={() => void command(async () => {
            setPreview(null); setInspection(await inspectDelivery(chatId, d)); setReason("");
          })}>Drain and inspect delivery #{d.id}</button>}
        </div>)}
      </section>}
      {inspection?.delivery.state === "inspection" && inspection.checkout && <form className="notice" aria-label="Keep inspected delivery" onSubmit={(event) => {
        event.preventDefault(); void command(async () => { const result = await acknowledgeDelivery(chatId, inspection, reason); setInspection(result); });
      }}>
        <h4>Keep this inspected state, without certifying delivery</h4>
        <p>Command groups have drained. This only releases the delivery's checkout reservation. The acknowledgement changes no file or ref, returns no lease and restarts no model.</p>
        {inspection.delivery.snapshot.action !== "integrate" && <p>Remote effects may still be unknown or delayed after the local command ended. Keeping this state does not certify that nothing was published.</p>}
        <p className="mono">{inspection.checkout.branch ?? "detached"} · {inspection.checkout.head}</p><pre>{inspection.checkout.status || "No tracked or untracked changes"}</pre>
        <label>Reason for keeping this outcome<input required maxLength={4000} value={reason} onChange={(e) => setReason(e.target.value)} /></label>
        <button className="button" disabled={busy || !reason.trim()}>Acknowledge and keep current state</button>
      </form>}
    </>}
  </section>;
}
