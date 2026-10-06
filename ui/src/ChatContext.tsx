import { lazy, Suspense } from "react";
import { ChatTeam, TEAM_PHASES } from "./ChatTeam";
import type { ChatDetail } from "./chat-api";
import type { RunEvent } from "./api";

const ChatChanges = lazy(() => import("./ChatChanges").then(module => ({ default: module.ChatChanges })));
const ChatPlanning = lazy(() => import("./ChatPlan").then(module => ({ default: module.ChatPlanning })));

export const CHAT_PANELS = {
  overview: "Overview",
  review: "Review",
  board: "Board",
  work: "Work",
} as const;
export type ChatPanel = keyof typeof CHAT_PANELS;

/** The command surfaces belong to this chat, never the project's latest run. */
export function ChatContext({ detail, panel, opened, hidden, status, tick, busy, command, events, elapsed, now, onChanged, onFeedback, onClose }: {
  detail: ChatDetail;
  panel: ChatPanel | null;
  opened: ReadonlySet<ChatPanel>;
  hidden: boolean;
  status: string;
  tick: number;
  busy: boolean;
  command: (act: () => Promise<unknown>) => Promise<void>;
  events: RunEvent[];
  elapsed: number;
  now: number;
  onChanged: () => void;
  onFeedback: (text: string) => void;
  onClose: () => void;
}) {
  const activeTeam = detail.turns.find(turn => turn.team?.control_node_id === detail.active_node_id)?.team;
  const running = detail.state === "running" || detail.state === "stopping";
  const activity = events.filter(event => event.node_run_id === (detail.active_node_id ?? detail.turns.at(-1)?.node.id));
  return <aside id="chat-context" className="chat-context" aria-label="Chat context" hidden={hidden || panel === null}
    onKeyDown={event => { if (event.key === "Escape" && !event.defaultPrevented) { event.stopPropagation(); onClose(); } }}>
    <header className="chat-context-heading">
      <h2>{panel ? CHAT_PANELS[panel] : "Chat context"}</h2>
      <button className="button" aria-label="Close chat panel" onClick={onClose}>×</button>
    </header>
    {/* Keep opened panels mounted: switching context must not discard review/plan drafts. */}
    {opened.has("overview") && <section className="chat-context-pane" aria-label="Overview" hidden={panel !== "overview"}>
      <section className="chat-overview-card">
        <h3>Execution</h3>
        <strong>{status}</strong>
        <p>{detail.mode === "team" ? "Pi team · chat-owned draft worktrees" : "Single Pi agent · local checkout"}</p>
        {running && <>
          <div className="chat-live-meter" role="progressbar" aria-label="Agent working" />
          <p className="mono">{Number.isFinite(elapsed) ? elapsed : 0}s elapsed</p>
        </>}
        <p>{activity.at(-1)?.summary ?? "Send a message to begin. No team or plan is required."}</p>
        <p className="faint">{detail.turns.length} {detail.turns.length === 1 ? "turn" : "turns"} in this conversation</p>
      </section>
      <section className="chat-overview-card">
        <h3>Working context</h3>
        <p className="mono">{detail.workspace_path}</p>
        <p>{detail.provider} / {detail.model}</p>
        <p className="faint">{activeTeam
          ? "This is the persistent solo checkout. Team builds use separate draft worktrees and never merge here automatically."
          : "Changes stay in this checkout. Completion is not an automatic commit or verification verdict."}</p>
      </section>
    </section>}
    {opened.has("review") && <section className="chat-context-pane" aria-label="Review" hidden={panel !== "review"}>
      <Suspense fallback={<p className="faint">Loading changes…</p>}>
        <ChatChanges chatId={detail.id} tick={tick} disabled={detail.archived || detail.active_node_id !== null} onChanged={onChanged} onFeedback={onFeedback} />
      </Suspense>
    </section>}
    {opened.has("board") && <section className="chat-context-pane" aria-label="Board" hidden={panel !== "board"}>
      <Suspense fallback={<p className="faint">Loading board…</p>}>
        <ChatPlanning chatId={detail.id} tick={tick} archived={detail.archived} frozen={activeTeam?.approved_revision != null} onChanged={onChanged} />
      </Suspense>
    </section>}
    {opened.has("work") && <section className="chat-context-pane" aria-label="Work" hidden={panel !== "work"}>
      {(detail.mode === "team" || detail.team_builds.length > 0) && <ChatTeam detail={detail} busy={busy} command={command} events={events} now={now} />}
      <section className="chat-overview-card">
        <h3>Turns</h3>
        {!detail.turns.length ? <p className="faint">No turns yet.</p> : <ol className="chat-turn-list">
          {detail.turns.map(turn => <li key={turn.run.id}>
            <span>{turn.run.prompt}</span>
            <span className="status" data-status={turn.team ? turn.run.status : turn.node.status}>{turn.team ? TEAM_PHASES[turn.team.phase] : turn.node.status}</span>
            <span className="faint">run #{turn.run.id} · {turn.node.model}</span>
          </li>)}
        </ol>}
      </section>
      <section className="chat-overview-card">
        <h3>Recent activity</h3>
        {!events.length ? <p className="faint">No activity yet.</p> : <ol className="chat-turn-list">
          {events.slice(-20).map(event => <li key={event.id}><span>{event.summary}</span><span className="faint">{event.at} · turn #{event.node_run_id}</span></li>)}
        </ol>}
      </section>
    </section>}
  </aside>;
}
