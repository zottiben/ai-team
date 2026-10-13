import { useEffect, useRef, useState } from "react";
import { BoardMarkdown } from "./BoardMarkdown";
import { cancelChatFollowup, sendChatFollowup, type ChatDetail, type ChatFollowup } from "./chat-api";
import type { RunEvent } from "./api";

// Animate new acknowledgements, not old history whenever polling or navigation renders it.
export function useFollowupArrivals(items: ChatFollowup[] | undefined) {
  const seen = useRef<Map<number, string> | null>(null);
  const timers = useRef(new Set<ReturnType<typeof setTimeout>>());
  const [arrivals, setArrivals] = useState<ReadonlySet<number>>(new Set());
  useEffect(() => {
    if (!items) return;
    for (const item of items) {
      if (item.state !== "delivered" || !seen.current || seen.current.get(item.id) === "delivered") continue;
      setArrivals(previous => new Set([...previous, item.id]));
      const timer = setTimeout(() => {
        setArrivals(previous => new Set([...previous].filter(id => id !== item.id)));
        timers.current.delete(timer);
      }, 900);
      timers.current.add(timer);
    }
    seen.current = new Map(items.map(item => [item.id, item.state]));
  }, [items]);
  useEffect(() => {
    const pending = timers.current;
    return () => { pending.forEach(clearTimeout); pending.clear(); };
  }, []);
  return arrivals;
}

// Detail and event polling can cross. This placeholder never enters the evidence stream.
export function followupEvent(item: ChatFollowup): RunEvent {
  return { id: -item.id, node_run_id: item.node_id, actor: "human", kind: "note", at: item.created_at, summary: "Queued message", message: item.body, thinking: [] };
}

export function FollowupReceipt({ item, status }: { item: ChatFollowup; status?: string }) {
  const progress = status === "done" ? "Turn finished" : status === "failed" ? "Turn failed" : status === "cancelled" ? "Turn stopped" : status === "running" ? "Turn running" : "Turn status unavailable";
  return <footer className="chat-message-receipt">
    <span role="status" title="Received means Pi echoed this exact prompt. Turn status is not verification that the requested change was completed.">{item.state === "delivered" ? `Received by agent · ${progress}` : "Delivery unconfirmed · inspect the response before retrying"}</span>
    {item.state === "starting" && <small>No automatic retry.{!!item.image_count && " Pi image resizing/conversion can prevent an exact acknowledgement."}</small>}
  </footer>;
}

export function QueuedInstructions({ detail, active, busy, command }: { detail: ChatDetail | null; active: boolean; busy: boolean; command: (action: () => Promise<unknown>) => Promise<void> }) {
  const items = detail?.followups?.filter(item => item.state === "queued") ?? [];
  if (!detail || !items.length) return null;
  return <section className="chat-followups chat-notice" aria-label="Queued instruction receipts">
    {items.map(item => <div key={item.id} className="chat-followup">
      <BoardMarkdown source={item.body}/>
      {!!item.image_count && <small>{item.image_count} image attachment(s)</small>}
      <small className="faint">Queued · not delivered · after turn #{item.after_node_id}</small>
      <div className="chat-controls">
        {!active && <button className="button" disabled={busy || detail.archived} onClick={() => void command(() => sendChatFollowup(detail.id, item.id))}>Send queued message</button>}
        <button className="button" disabled={busy || detail.archived} onClick={() => void command(() => cancelChatFollowup(detail.id, item.id))}>Cancel queued message</button>
      </div>
    </div>)}
    {!active && <p className="faint">This instruction is held. Send it explicitly or cancel it; restarting the app does not send it.</p>}
  </section>;
}

export function CancelledInstructions({ items }: { items: ChatFollowup[] | undefined }) {
  const cancelled = items?.filter(item => item.state === "cancelled") ?? [];
  if (!cancelled.length) return null;
  return <details className="chat-cancelled"><summary>Cancelled queued messages ({cancelled.length}) · not delivered</summary>
    {cancelled.map(item => <div className="chat-followup" key={item.id}><BoardMarkdown source={item.body}/><small>After turn #{item.after_node_id}{!!item.image_count && ` · ${item.image_count} image attachment(s)`}</small></div>)}
  </details>;
}
