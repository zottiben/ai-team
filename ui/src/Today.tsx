import { useEffect, useState } from "react";
import {
  chatToday,
  type ChatToday,
  type ChatTodayEntry,
} from "./chat-today-api";
import type { ChatPanel } from "./ChatContext";

const LABELS: Record<string, string> = {
  empty: "New chat",
  idle: "Idle",
  starting: "Starting / waiting",
  running: "Working",
  stopping: "Stopping",
  interrupted: "Interrupted",
  failed: "Failed",
  blocked: "Blocked",
  stopped: "Stopped",
  awaiting_approval: "Build approval",
  review: "Drafts to review",
  inspection: "Needs inspection",
  checkout_change: "Checkout change",
  queued_followup: "Queued instruction",
  questions: "Plan questions",
};
type OpenChat = (project: string, chat: number, panel: ChatPanel) => void;

/** One authoritative chat-only response supplies the queue and every counter.
 * Legacy /today, /runs and /analytics are intentionally not supporting feeds here. */
export function Today({
  tick,
  onOpenChat,
}: {
  tick: number;
  onOpenChat: OpenChat;
}) {
  const [data, setData] = useState<ChatToday | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    let sequence = 0;
    const load = () => {
      const request = ++sequence;
      void chatToday()
        .then((next) => {
          if (current && request === sequence) {
            setData(next);
            setProblem(null);
          }
        })
        .catch((error: unknown) => {
          if (current && request === sequence)
            setProblem(error instanceof Error ? error.message : String(error));
        });
    };
    load();
    // A supervisor dying is observable even before another event reaches SQLite.
    const timer = setInterval(load, 5000);
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, [tick]);
  if (problem)
    return (
      <p className="error" role="alert">
        {problem}
      </p>
    );
  if (!data) return <p className="empty">Loading chat activity…</p>;
  const attention = data.entries.filter((entry) => entry.needs_attention);
  const rest = data.entries.filter((entry) => !entry.needs_attention);
  const projects = [
    ...new Set(data.entries.map((entry) => entry.project_slug)),
  ];
  return (
    <div className="today">
      <header className="today__masthead">
        <div>
          <span className="eyebrow">Chat activity</span>
          <h2>Today</h2>
          <p className="faint">
            Current work and attention across your chats. Archived chats and
            pre-refactor run history are excluded.
          </p>
        </div>
      </header>
      <section className="today__pulse" aria-label="Chat activity counts">
        <Metric
          value={data.needs_attention}
          label="Needs you"
          tone="attention"
        />
        <Metric value={data.working} label="Chats working" tone="live" />
        <Metric value={data.drafts} label="Drafts to review" tone="review" />
        <Metric value={data.chats} label="Open chats" tone="quiet" />
      </section>
      <div className="today__layout">
        <div className="today__primary">
          {attention.length > 0 ? (
            <section className="today__queue">
              <div className="today__section-heading">
                <div>
                  <span className="eyebrow">Needs your attention</span>
                  <h3>Do this first</h3>
                </div>
              </div>
              <div className="list">
                {attention.map((entry) => (
                  <Row
                    key={entry.chat_id}
                    entry={entry}
                    onOpenChat={onOpenChat}
                  />
                ))}
              </div>
            </section>
          ) : (
            <section className="today__clear">
              <span className="today__clear-mark" aria-hidden="true">
                ✓
              </span>
              <div>
                <h3>Nothing is waiting on you</h3>
                <p className="faint">
                  {data.chats
                    ? "Your open chats are shown below."
                    : "Start a chat in a project. Its work and results will appear here."}
                </p>
              </div>
            </section>
          )}
          {rest.length > 0 && (
            <section className="today__queue">
              <h3>Other open chats</h3>
              <div className="list">
                {rest.map((entry) => (
                  <Row
                    key={entry.chat_id}
                    entry={entry}
                    onOpenChat={onOpenChat}
                  />
                ))}
              </div>
            </section>
          )}
          {data.entries.length < data.chats && (
            <p className="notice">
              Showing {data.entries.length} of {data.chats} open chats,
              attention first. Counts cover all chats.
            </p>
          )}
        </div>
        <aside className="today__rail">
          <section className="today-panel">
            <div className="today__section-heading">
              <div>
                <span className="eyebrow">Across your chats</span>
                <h3>Projects</h3>
              </div>
            </div>
            {projects.length === 0 ? (
              <p className="faint">No projects with open chats.</p>
            ) : (
              <div className="today-projects">
                {projects.map((slug) => {
                  const entries = data.entries.filter(
                    (entry) => entry.project_slug === slug,
                  );
                  return (
                    <div className="today-project" key={slug}>
                      <strong>{entries[0]?.project_name ?? slug}</strong>
                      <span className="faint">
                        {entries.length} shown ·{" "}
                        {
                          entries.filter((entry) => entry.needs_attention)
                            .length
                        }{" "}
                        need attention
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
          </section>
          <section className="today-panel">
            <div className="today__section-heading">
              <h3>Recent chat activity</h3>
            </div>
            <div className="today-recent">
              {[...data.entries]
                .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
                .slice(0, 4)
                .map((entry) => (
                  <button
                    type="button"
                    key={entry.chat_id}
                    onClick={() =>
                      onOpenChat(entry.project_slug, entry.chat_id, entry.panel)
                    }
                  >
                    <span>
                      <strong>{entry.title}</strong>
                      <span className="faint">
                        {entry.project_name} ·{" "}
                        {LABELS[entry.state] ?? entry.state}
                      </span>
                    </span>
                    <span aria-hidden="true">→</span>
                  </button>
                ))}
            </div>
          </section>
        </aside>
      </div>
    </div>
  );
}
function Metric({
  value,
  label,
  tone,
}: {
  value: number;
  label: string;
  tone: string;
}) {
  return (
    <div className="today-metric" data-tone={value ? tone : "quiet"}>
      <strong>{value}</strong>
      <span>{label}</span>
    </div>
  );
}
function Row({
  entry,
  onOpenChat,
}: {
  entry: ChatTodayEntry;
  onOpenChat: OpenChat;
}) {
  return (
    <button
      type="button"
      className="card today-row"
      onClick={() => onOpenChat(entry.project_slug, entry.chat_id, entry.panel)}
    >
      <div className="today-row__body">
        <div className="today-row__meta">
          <span
            className="status"
            data-status={
              entry.working
                ? "running"
                : entry.needs_attention
                  ? "blocked"
                  : "done"
            }
          >
            {LABELS[entry.state] ?? entry.state}
          </span>
          <span className="faint">
            {entry.project_name} · chat {entry.chat_id}
          </span>
        </div>
        <strong>{entry.title}</strong>
        {entry.detail && <span className="faint">{entry.detail}</span>}
        {entry.questions > 0 && (
          <span>{entry.questions} open plan questions</span>
        )}
        {entry.drafts > 0 && (
          <span>
            {entry.drafts} recorded verified drafts · review is not publication
            approval
          </span>
        )}
        <span className="faint mono">{entry.workspace_path}</span>
      </div>
      <span aria-hidden="true">→</span>
    </button>
  );
}
