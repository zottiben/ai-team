import { lazy, Suspense, useEffect, useRef, useState } from "react";
const ChatCheckout = lazy(() => import("./ChatCheckout").then(m => ({ default: m.ChatCheckout })));
import { ExactApproval } from "./synth/views/Review/ExactApproval";
import { DraftReviewPanel } from "./synth/views/Review/DraftReview";
import { captureReview, currentReview, deliveryActions as ACTIONS, openReviewView, reviewDraftState, type ReviewViews } from "./synth/views/Review/drafts";
import { useReviewFix } from "./useReviewFix";
import { PanelLoading } from "./PanelLoading";
import {
  acknowledgeDelivery, approveDelivery, chatChanges, inspectDelivery, previewDelivery, reviewDraft,
  type Changes, type Delivery, type DeliveryInspection,
} from "./changes-api";
import "./chat-changes.css";

type Props = { chatId: number; mode: "single" | "team"; tick: number; disabled: boolean; working?: boolean; disabledReason?: string; onChanged: () => void; onFeedback?: (text: string) => void };
export function ChatChanges(props: Props) { return <ChangesController key={props.chatId} {...props}/>; }
function ChangesController({ chatId, mode, tick, disabled, working, disabledReason, onChanged, onFeedback }: Props) {
  const submitFix = useReviewFix(chatId);
  const host = useRef<HTMLElement>(null);
  const [changes, setChanges] = useState<Changes | null>(null);
  const [views, setViews] = useState<ReviewViews>({ active: null, sessions: [] });
  const [preview, setPreview] = useState<Delivery | null>(null);
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
  const selectedView = views.sessions.find(session => session.key === views.active), selected = selectedView?.review.draft;
  const fresh = selectedView && changes && currentReview(selectedView, changes);
  const approvalCurrent = !!preview && !!changes && !!selected && !!fresh && preview.chat_id === chatId && preview.state === "preview"
    && preview.snapshot.workspace_epoch === changes.workspace_epoch && preview.snapshot.workspace_path === changes.workspace_path
    && preview.snapshot.checkout_head === changes.head && preview.snapshot.checkout_branch === changes.branch
    && preview.snapshot.commit_sha === selected.commit_sha && preview.snapshot.target.run_id === selected.target.run_id
    && preview.snapshot.target.slice_key === selected.target.slice_key && preview.snapshot.target.revision === selected.target.revision;
  return <section ref={host} tabIndex={-1} className="chat-overview-card chat-changes" aria-label="Changes and delivery">
    <div className="chat-changes-heading"><h3>Changes and delivery</h3>
      <button className="button" disabled={busy} onClick={() => { setProblem(null); setRefresh((n) => n + 1); }}>Refresh changes</button></div>
    {problem && <p role="alert" className="error">{problem}</p>}
    {!changes ? problem ? <p className="faint">Changes unavailable. Refresh to retry.</p> : <PanelLoading>Reading actual Git changes…</PanelLoading> : <>
      {changes.issues.length > 0 && <div className="notice"><strong>Checkout or draft inspection is incomplete</strong><ul>{changes.issues.map((issue, n) => <li key={n}>{issue}</li>)}</ul></div>}
      <details open>
        <summary>Working checkout · {changes.branch ?? "detached"} · {changes.staged.length} staged · {changes.unstaged.length} unstaged · {changes.untracked.length} untracked</summary>
        <Suspense fallback={<p>Loading checkout actions…</p>}><ChatCheckout chatId={chatId} mode={mode} tick={tick+refresh} disabled={disabled||!!pending} working={working} otherBusy={busy}
          disabledReason={disabled?disabledReason:pending?"A draft delivery is unresolved. Inspect it before changing the checkout.":undefined} onChanged={onChanged} onFeedback={onFeedback}/></Suspense>
      </details>
      <h4>Verified drafts from this chat</h4>
      <p className="faint">Verification is not integration or publication. Each action needs its own exact approval. Draft review reads commit objects, not a returned worktree.</p>
      {!changes.drafts.length && <p>No verified draft commits yet.</p>}
      {changes.drafts.map((draft) => <div className="chat-draft" key={`${draft.target.run_id}:${draft.target.slice_key}`}>
        <strong>{draft.target.slice_key} · run #{draft.target.run_id}</strong>
        <span className="mono">{draft.commit_sha}</span>
        <span className="faint">{draft.branch} · lease {draft.lease_state}</span>
        <button className="button" disabled={busy} onClick={() => void command(async () => {
          const scope = { epoch: changes.workspace_epoch, workspace: changes.workspace_path };
          const next = captureReview(await reviewDraft(chatId, draft.target), scope);
          setViews(previous => openReviewView(previous, next)); setPreview(null);
        })}>Review {draft.target.slice_key}</button>
      </div>)}
      {views.sessions.some(session => session.key !== views.active) && <nav aria-label="Retained draft reviews">
        <p className="faint">Other snapshots keep their open comments and local findings. Returning does not rebind them to current files or approve a repair.</p>
        {views.sessions.filter(session => session.key !== views.active).map(session => <button type="button" className="button" key={session.key} disabled={busy}
          onClick={() => { setViews(previous => openReviewView(previous, session)); setPreview(null); }}>Return to retained review {session.review.draft.target.slice_key} · run {session.review.draft.target.run_id} · revision {session.review.draft.target.revision} · generation {session.epoch}</button>)}
      </nav>}
      {views.sessions.map(session => <DraftReviewPanel key={session.key} chatId={chatId} session={session} active={session.key === views.active}
        changes={changes} mode={mode} unavailable={!!unavailable} busy={busy} disabled={disabled} command={command} submitFix={submitFix}
        onDirty={dirty => setViews(previous => reviewDraftState(previous, session.key, dirty))}
        onPreview={action => void command(async () => { setPreview(await previewDelivery(chatId, session.review.draft.target, action)); })}/>)}
      {preview && <ExactApproval family="delivery" receipt={preview} label="Delivery approval" returnFocus={host} summary={<>
        <h4>{ACTIONS[preview.snapshot.action]} — exact approval</h4>
        <dl>
          <dt>Draft</dt><dd className="mono">{preview.snapshot.commit_sha}</dd>
          <dt>Scope</dt><dd>run #{preview.snapshot.target.run_id} · {preview.snapshot.target.slice_key} · draft revision {preview.snapshot.target.revision} · checkout generation {preview.snapshot.workspace_epoch ?? "unrecorded"}</dd>
          <dt>Checkout</dt><dd className="mono">{preview.snapshot.workspace_path} · {preview.snapshot.checkout_branch} · {preview.snapshot.checkout_head}</dd>
          {preview.snapshot.remote_url && <><dt>Destination</dt><dd className="mono">{preview.snapshot.remote_url} · {preview.snapshot.delivery_branch}</dd></>}
          {preview.snapshot.base_branch && <><dt>PR base</dt><dd className="mono">{preview.snapshot.github_repo} · {preview.snapshot.base_branch} · {preview.snapshot.base_sha}</dd></>}
        </dl>
        <p>{preview.snapshot.action === "integrate"
          ? "Fast-forward this clean checkout only. Dirty files or divergent history cause refusal; nothing is stashed, reset or cherry-picked."
          : preview.snapshot.action === "push" ? "Publish only this commit to the named ref. Do not open or merge a PR."
            : "Create an unmerged draft PR for the already published commit. Do not retarget an existing PR or request a merge."}</p>
      </>}>{ready => <>
        {unavailable && <p className="notice">{disabled ? disabledReason ?? "Finish or close the current execution before delivery." : busy ? "An existing request is pending. No additional delivery action is available." : "A draft delivery is unresolved. Inspect it before another action."}</p>}
        {!approvalCurrent && <p className="notice">{preview.snapshot.workspace_epoch == null ? "Checkout generation was not recorded; request a fresh preview." : "Changed since preview — review again. This approval no longer matches the current checkout generation and draft."}</p>}
        <button className="button button--primary" disabled={unavailable || !approvalCurrent || !ready} onClick={() => void command(async () => {
          const result = await approveDelivery(chatId, preview); setPreview(null);
          if (result.state !== "done") throw new Error(result.result ?? "Delivery did not finish; inspect its recorded state.");
        })}>Approve: {ACTIONS[preview.snapshot.action]}</button>
        <button className="button" disabled={busy} onClick={() => setPreview(null)}>Dismiss delivery</button>
      </>}</ExactApproval>}
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
