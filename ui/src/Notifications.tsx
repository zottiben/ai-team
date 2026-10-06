import { useCallback, useEffect, useRef, useState } from "react";
import { Popover } from "./Popover";
import { notificationInbox, readNotification, readNotifications, clearNotification, clearNotifications, type Notification } from "./api";

/** Inbox changes never change authoritative chat/run state. */
export function Notifications({ tick, onOpen }: {
  tick: number;
  onOpen: (notification: Notification) => void;
}) {
  const [items, setItems] = useState<Notification[]>([]);
  const [unread, setUnread] = useState(0);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const generation = useRef(0);
  const saving = useRef(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; generation.current++; };
  }, []);
  const load = useCallback(async () => {
    const request = ++generation.current;
    try {
      const inbox = await notificationInbox();
      if (!alive.current || request !== generation.current) return;
      setItems(inbox.items);
      setUnread(inbox.unread);
    } catch (error: unknown) {
      if (alive.current && request === generation.current)
        setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);
  useEffect(() => { void load(); }, [load, tick]);

  const change = async (action: () => Promise<unknown>, after?: () => void) => {
    if (saving.current) return;
    saving.current = true;
    generation.current++;
    setBusy(true);
    setProblem(null);
    try {
      await action();
      await load();
      if (alive.current) after?.();
    } catch (error: unknown) {
      if (alive.current) setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      saving.current = false;
      if (alive.current) setBusy(false);
    }
  };
  // Bound bulk actions to this snapshot; arrivals after it must not be cleared unseen.
  const throughId = items[0]?.id;
  return <div className="notification-center">
    <button ref={trigger} type="button" className="nav-item" aria-expanded={open}
      aria-label={unread ? `Notifications · ${unread} unread` : "Notifications"}
      title="Notifications" onClick={() => setOpen(value => !value)}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4" />
      </svg>
      {unread > 0 && <span className="notification-center__count">{unread}</span>}
    </button>
    {open && <Popover anchor={trigger} label="Notifications" className="notification-center__panel" onClose={() => setOpen(false)}>
      <div className="block__head">
        <h3>Notifications</h3>
        <span className="faint">{unread === 0 ? "all caught up" : `${unread} unread`}</span>
        <button className="button" aria-label="Close notifications" onClick={() => { setOpen(false); trigger.current?.focus(); }}>×</button>
      </div>
      <div className="notification-center__actions">
        <button className="button" disabled={busy || !unread || throughId === undefined} onClick={() => void change(() => readNotifications(throughId!))}>Mark all as read</button>
        <button className="button" disabled={busy || throughId === undefined} onClick={() => void change(() => clearNotifications(throughId!))}>Clear all</button>
      </div>
      {problem !== null && <p className="error" role="alert">{problem} <button className="button" disabled={busy} onClick={() => { setProblem(null); void load(); }}>Refresh</button></p>}
      {items.length === 0 ? <p className="empty">No notifications. New completions and requests for input will appear here.</p> : <div className="notification-center__list">
        {items.map(item => <article key={item.id} className="notification-center__item" data-read={item.read_at !== null}>
          <button className="notification-center__open" disabled={busy} onClick={() => void change(
            () => item.read_at === null ? readNotification(item.id) : Promise.resolve(),
            () => { setOpen(false); onOpen(item); },
          )}>
            <span className="workspace-dot" data-status={statusOf(item.kind)} />
            <span><strong>{item.title}</strong><span>{item.body}</span><time className="faint" dateTime={item.created_at}>{when(item.created_at)}</time></span>
          </button>
          <div className="notification-center__actions">
            {item.read_at === null && <button className="button" disabled={busy} aria-label={`Mark as read: ${item.title}`} onClick={() => void change(() => readNotification(item.id))}>Mark as read</button>}
            <button className="button" disabled={busy} aria-label={`Clear: ${item.title}`} onClick={() => void change(() => clearNotification(item.id))}>Clear</button>
          </div>
        </article>)}
      </div>}
      <small className="faint notification-center__hint">Clearing hides alerts, not chat history. Showing up to 60 recent items; bulk actions include older alerts.</small>
    </Popover>}
  </div>;
}

function statusOf(kind: Notification["kind"]): string {
  if (kind === "failed") return "failed";
  if (kind === "input_required" || kind === "follow_up" || kind === "plan_ready") return "parked";
  return "done";
}

function when(value: string): string {
  const at = Date.parse(value);
  if (Number.isNaN(at)) return value;
  const seconds = Math.max(0, Math.round((Date.now() - at) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`;
  return new Date(at).toLocaleDateString();
}
