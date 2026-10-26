import { useEffect, useRef, useState } from "react";
import type { CheckoutState } from "../../../checkout-api";
import { Lcd } from "../../primitives/Lcd";
import { Led } from "../../primitives/Led";
import { Plate } from "../../primitives/Plate";
import { Silk } from "../../primitives/Silk";
import { useMotionVisible } from "../../motion/useMotionVisible";
import { latestOperation, pushedCommit, receiptScope } from "./data";

function Link({ available }: { available: boolean }) {
  const host = useRef<SVGSVGElement>(null), motion = useMotionVisible(host);
  return <svg ref={host} className="synth-review-link" viewBox="0 0 24 24" aria-hidden="true" data-open={available} data-motion={motion}>
    <path className="synth-review-link__closed" d="M3 12 C9 18 15 18 21 12"/>
    <path className="synth-review-link__open" pathLength={1} d="M3 12 C9 18 15 18 21 12"/>
    <circle cx={3} cy={12} r={2}/><circle cx={21} cy={12} r={2}/>
  </svg>;
}

function useReceiptFlash(scope: string, count: string, open: boolean) {
  const prior = useRef({ scope, count, open }), sequence = useRef(0);
  const [event, setEvent] = useState<number>();
  useEffect(() => {
    const last = prior.current; prior.current = { scope, count, open };
    if (last.scope !== scope) setEvent(undefined);
    else if (last.count !== count || (!last.open && open)) setEvent(++sequence.current);
  }, [scope, count, open]);
  return event;
}

/** Only describes existing controls and receipts. It cannot approve or dispatch an action. */
export function DeliveryChain({ data, locked, working, commitMessage }: {
  data: CheckoutState; locked: string | null; working: boolean; commitMessage: string;
}) {
  const [copyProblem, setCopyProblem] = useState<string | null>(null);
  const push = latestOperation(data.operations, ["push", "push_branch"]), pr = latestOperation(data.operations, ["pull_request"]);
  const pushScope = push ? receiptScope(push, data) : null;
  const pushCommit = push ? pushedCommit(push) : null;
  const sameCommit = !!pushCommit && pushCommit === data.head;
  const pushText = !push ? "Unknown" : push.state === "done" ? "Recorded" : push.state;
  const scope = JSON.stringify([data.workspace, data.workspace_epoch]);
  const checkoutEvent = useReceiptFlash(scope, `${data.unstaged.length}:${data.untracked.length}`, !locked);
  const indexEvent = useReceiptFlash(scope, String(data.staged.length), !locked);
  return <Plate tone="night" className="synth-review-delivery" heading="Checkout & delivery">
    <Lcd className="synth-review-branch">
      <strong>{data.branch ?? "Detached HEAD"}</strong>
      <code title={data.head ?? undefined} aria-label={`HEAD ${data.head ?? "unborn"}`}>{data.head?.slice(0, 12) ?? "Unborn HEAD"}</code>
      <button type="button" className="button" disabled={!data.head} onClick={() => {
        const head = data.head;
        if (!head) return;
        setCopyProblem(null);
        void (async () => {
          if (!navigator.clipboard) throw Error("Clipboard is unavailable; select the full hash in checkout identity.");
          await navigator.clipboard.writeText(head);
        })().catch((error: unknown) => setCopyProblem(String(error)));
      }}>Copy HEAD</button>
      <span className="synth-review-path" title={data.workspace}><bdi dir="ltr">{data.workspace}</bdi></span>
    </Lcd>
    {copyProblem && <p role="status">{copyProblem}</p>}
    <ol className="synth-review-chain" aria-label="Independent checkout delivery stages">
      <li><Lcd><Silk>Checkout</Silk><strong>{data.unstaged.length}</strong><span>unstaged</span><small>{data.untracked.length} untracked</small><Led state={checkoutEvent === undefined ? "on" : "event"} eventId={checkoutEvent} signal="diff"/></Lcd>
        <Link available={!locked && data.unstaged.length + data.untracked.length > 0}/></li>
      <li><Lcd><Silk>Index</Silk><strong>{data.staged.length}</strong><span>staged</span><small>{locked ? "Locked" : "Preview first"}</small><Led state={working ? "pulse" : indexEvent === undefined ? "on" : "event"} eventId={indexEvent} signal={locked ? "attention" : "diff"}/></Lcd>
        <Link available={!locked && data.staged.length > 0 && !!commitMessage.trim()}/></li>
      <li><Lcd><Silk>Commit</Silk><strong className="synth-review-head" title={data.head ?? undefined}>{data.head?.slice(0, 7) ?? "—"}</strong><span>snapshot HEAD</span><small>Approval required</small><Led state={data.head ? "on" : "off"} signal="diff"/></Lcd>
        <Link available={!locked && !!data.head && !!data.branch}/></li>
      <li><Lcd><Silk>Push</Silk><strong className="synth-review-push-state">{pushText}</strong><span>{push ? `receipt #${push.id}` : "grant unknown"}</span>
        <small>{push ? pushScope === "current" ? sameCommit ? "this HEAD" : "other commit" : pushScope === "prior" ? "prior generation" : "generation unknown" : "No push receipt"}</small>
        <Led state={push ? "on" : "off"} signal={push?.state === "done" ? pushScope === "current" && sameCommit ? "done" : "info" : "attention"}/></Lcd></li>
    </ol>
    {locked && <p className="notice" role="note">{locked}{working && " Counts are the retained inspection snapshot, not live edit progress."}</p>}
    <details><summary>Checkout identity and publication evidence</summary>
      <p className="faint synth-review-explainer">Links show preview availability, not approval. Each action is rechecked separately; push does not create a PR.</p>
      <dl><dt>Workspace generation</dt><dd>{data.workspace_epoch}</dd><dt>Full HEAD</dt><dd className="mono">{data.head ?? "Unborn"}</dd><dt>Checkout fingerprint</dt><dd className="mono">{data.fingerprint}</dd></dl>
      {push ? <p className="mono">Latest push receipt #{push.id}: {push.state} · {pushedCommit(push) ?? "commit unrecorded"} · {push.snapshot.remote?.branch ?? "destination unrecorded"}</p>
        : <p>No push receipt in this page of history. A grant in a human chat message may still exist; this view does not inspect or revoke it.</p>}
      {pr ? <p>Latest draft-PR receipt #{pr.id}: {pr.state} · {pr.result ?? "outcome not recorded"}</p> : <p>No draft-PR receipt in this page of history. A push does not imply a PR.</p>}
      <p className="faint">Up to 100 checkout receipts, unresolved first. Recorded success is not a live remote check or current publication authority. Full operation results remain below.</p>
    </details>
  </Plate>;
}
