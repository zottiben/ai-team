import { lazy, Suspense, useRef } from "react";
import { TEAM_PHASES } from "./team-api";
import type { ChatDetail } from "./chat-api";
import type { RunEvent } from "./api";

const PanelSelector = lazy(() => import("./synth/views/Session/SessionNavigation").then(m => ({ default: m.PanelSelector })));
const ChatChanges = lazy(() => import("./ChatChanges").then(module => ({ default: module.ChatChanges })));
const ChatOverview = lazy(() => import("./ChatOverview").then(module => ({ default: module.ChatOverview })));
const ChatPlanning = lazy(() => import("./ChatPlan").then(module => ({ default: module.ChatPlanning })));
const ChatTeam = lazy(() => import("./ChatTeam").then(module => ({ default: module.ChatTeam })));
const WorkPanel = lazy(() => import("./synth/views/panels/WorkPanel"));

export const CHAT_PANELS = {
  overview: "Overview",
  review: "Review",
  board: "Board",
  work: "Work",
} as const;
export type ChatPanel = keyof typeof CHAT_PANELS;

/** The command surfaces belong to this chat, never the project's latest run. */
export function ChatContext({ detail, panel, opened, hidden, status, tick, busy, command, events, elapsed, now, onChanged, onFeedback, onClose, synth = false, onSelect }: {
  synth?: boolean;
  onSelect?: (panel: ChatPanel) => void;
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
  const workOpened = useRef(false);
  if (synth && opened.has("work")) workOpened.current = true;
  return <aside id="chat-context" className="chat-context" aria-label="Chat context" hidden={hidden || panel === null}
    onKeyDown={event => { if (event.key === "Escape" && !event.defaultPrevented) { event.stopPropagation(); onClose(); } }}>
    <header className="chat-context-heading">
      {synth && panel && panel !== "board" && onSelect ? <Suspense fallback={<h2>{CHAT_PANELS[panel]}</h2>}><PanelSelector panel={panel} onChange={onSelect} /></Suspense> : <h2>{panel ? CHAT_PANELS[panel] : "Chat context"}</h2>}
      <button className="button" aria-label="Close chat panel" onClick={onClose}>×</button>
    </header>
    {/* Keep opened panels mounted: switching context must not discard review/plan drafts. */}
    {opened.has("overview") && <section id="chat-pane-overview" role={synth ? "tabpanel" : undefined} className="chat-context-pane" aria-label="Overview" hidden={panel !== "overview"}>
      <Suspense fallback={<p className="faint">Loading overview…</p>}>
        <ChatOverview detail={detail} status={status} tick={tick} events={events} elapsed={elapsed} now={now} synth={synth} onSelect={onSelect} />
      </Suspense>
    </section>}
    {opened.has("review") && <section id="chat-pane-review" role={synth ? "tabpanel" : undefined} className="chat-context-pane" aria-label="Review" hidden={panel !== "review"}>
      <Suspense fallback={<p className="faint">Loading changes…</p>}>
        <ChatChanges chatId={detail.id} mode={detail.mode} tick={tick} disabled={detail.archived || detail.active_node_id !== null}
          working={detail.state === "running" || detail.state === "stopping"}
          disabledReason={detail.archived ? "Archived chat — checkout actions are read-only." : undefined} onChanged={onChanged} onFeedback={onFeedback} />
      </Suspense>
    </section>}
    {opened.has("board") && <section id="chat-pane-board" role={synth ? "tabpanel" : undefined} className="chat-context-pane" aria-label="Board" hidden={panel !== "board"}>
      <Suspense fallback={<p className="faint">Loading board…</p>}>
        <ChatPlanning chatId={detail.id} tick={tick} archived={detail.archived} frozen={activeTeam?.approved_revision != null} onChanged={onChanged} />
      </Suspense>
    </section>}
    {opened.has("work") && <section id="chat-pane-work" role={synth ? "tabpanel" : undefined} className="chat-context-pane" aria-label="Work" hidden={panel !== "work"}>
      {/* Team execution is rendered in the same place for both skins: a review, a close
          form or a retained inspection must survive the flag, not be remounted by it. */}
      {(detail.mode === "team" || detail.team_builds.length > 0) && <Suspense fallback={<p className="faint">Loading team execution…</p>}><ChatTeam detail={detail} busy={busy} command={command} events={events} now={now} /></Suspense>}
      <div hidden={!synth}>{workOpened.current && <Suspense fallback={<p className="faint">Loading work…</p>}>
        <WorkPanel detail={detail} events={events} now={now} visible={synth && panel === "work" && !hidden} />
      </Suspense>}</div>
      <div hidden={synth}>
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
            {events.filter(event => !event.telemetry_only).slice(-20).map(event => <li key={event.id}><span>{event.summary}</span><span className="faint">{event.at} · turn #{event.node_run_id}</span></li>)}
          </ol>}
        </section>
      </div>
    </section>}
  </aside>;
}
