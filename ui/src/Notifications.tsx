import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";

import {
  notifications as fetchNotifications,
  readNotification,
  type Notification,
} from "./api";

/** One window-wide inbox for state transitions that deserve the operator's attention. */
export function Notifications({
  tick,
  onOpen,
}: {
  tick: number;
  onOpen: (notification: Notification) => void;
}) {
  const [items, setItems] = useState<Notification[]>([]);
  const [open, setOpen] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setItems(await fetchNotifications());
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, tick]);

  const unread = items.filter((item) => item.read_at === null).length;
  const choose = async (item: Notification) => {
    if (item.read_at === null) {
      try {
        const read = await readNotification(item.id);
        setItems((current) => current.map((entry) => (entry.id === read.id ? read : entry)));
      } catch (error: unknown) {
        setProblem(error instanceof Error ? error.message : String(error));
        return;
      }
    }
    setOpen(false);
    onOpen(item);
  };

  return (
    <div className="notification-center">
      <button
        type="button"
        className="nav-item"
        aria-expanded={open}
        aria-label={unread ? `Notifications · ${unread} unread` : "Notifications"}
        title="Notifications"
        onClick={() => setOpen((value) => !value)}
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4" />
        </svg>
        {unread > 0 && <span className="notification-center__count">{unread}</span>}
      </button>

      {open &&
        createPortal(
          <section className="notification-center__panel" aria-label="Notifications">
            <div className="block__head">
              <h3>Notifications</h3>
              <span className="faint">{unread === 0 ? "all caught up" : `${unread} unread`}</span>
            </div>
            {problem !== null && <p className="error">{problem}</p>}
            {items.length === 0 ? (
              <p className="empty">Agent completions and requests for input appear here.</p>
            ) : (
              <div className="notification-center__list">
                {items.map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    className="notification-center__item"
                    data-read={item.read_at !== null}
                    onClick={() => void choose(item)}
                  >
                    <span className="workspace-dot" data-status={statusOf(item.kind)} />
                    <span>
                      <strong>{item.title}</strong>
                      <span>{item.body}</span>
                      <time className="faint" dateTime={item.created_at}>
                        {when(item.created_at)}
                      </time>
                    </span>
                  </button>
                ))}
              </div>
            )}
          </section>,
          document.body,
        )}
    </div>
  );
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
