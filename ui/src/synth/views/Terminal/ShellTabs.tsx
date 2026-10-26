import { useRef } from "react";

import type { TerminalSession } from "../../../api";
import { Led } from "../../primitives/Led";

export const AGENT_TAB = "agent";

/**
 * Your shells, and the agent's (§12.1).
 *
 * The agent's tab is a different kind of thing from the rest - it has no session behind
 * it - so it is marked as one: a lock, a read-only label, and no close. Its LED pulses
 * while a command is actually in flight, which is evidence from the event feed rather
 * than a guess from how recently something was printed.
 */
export function ShellTabs({
  panelId,
  sessions,
  selected,
  agentRunning,
  busy,
  onSelect,
  onOpen,
  onClose,
}: {
  panelId: string;
  sessions: readonly TerminalSession[];
  selected: string;
  agentRunning: boolean;
  busy: boolean;
  onSelect: (id: string) => void;
  onOpen: () => void;
  onClose: (session: number) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const ids = [...sessions.map((session) => String(session.id)), AGENT_TAB];
  const focusable = ids.includes(selected) ? selected : ids[0];
  const shells = sessions.filter((session) => session.kind === "shell");
  return (
    <div
      ref={host}
      className="synth-shell-tabs"
      role="tablist"
      aria-label="Terminal sessions"
      onKeyDown={(event) => {
        const buttons = [...(host.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]') ?? [])];
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        if (index < 0 || !buttons.length) return;
        const target =
          event.key === "ArrowRight"
            ? (index + 1) % buttons.length
            : event.key === "ArrowLeft"
              ? (index + buttons.length - 1) % buttons.length
              : event.key === "Home"
                ? 0
                : event.key === "End"
                  ? buttons.length - 1
                  : -1;
        if (target >= 0) {
          event.preventDefault();
          buttons[target]?.focus();
        }
      }}
    >
      {sessions.map((session) => (
        <span key={session.id} className="synth-shell-tabs__tab">
          <button
            type="button"
            role="tab"
            id={`${panelId}-tab-${session.id}`}
            className="synth-key mono"
            aria-selected={selected === String(session.id)}
            aria-controls={panelId}
            tabIndex={focusable === String(session.id) ? 0 : -1}
            onClick={() => onSelect(String(session.id))}
          >
            <Led state={session.done ? "off" : "on"} signal={session.kind === "command" ? "working" : "done"} size={6} />
            {session.kind === "command" ? `run · ${session.label}` : `shell ${shells.indexOf(session) + 1}`}
            {session.done && ` · exited ${session.status ?? "?"}`}
          </button>
          <button
            type="button"
            className="synth-key synth-shell-tabs__close"
            disabled={busy}
            aria-label={`Close ${session.kind === "command" ? session.label : `shell ${shells.indexOf(session) + 1}`}`}
            onClick={() => onClose(session.id)}
          >
            ×
          </button>
        </span>
      ))}
      <button
        type="button"
        role="tab"
        id={`${panelId}-tab-${AGENT_TAB}`}
        className="synth-key mono"
        aria-selected={selected === AGENT_TAB}
        aria-controls={panelId}
        tabIndex={focusable === AGENT_TAB ? 0 : -1}
        onClick={() => onSelect(AGENT_TAB)}
      >
        <Led state={agentRunning ? "pulse" : "off"} signal="working" size={6} />
        <span aria-hidden="true">🔒</span> agent · bash (read-only mirror)
      </button>
      <button type="button" className="synth-key" disabled={busy} onClick={onOpen} aria-label="Open another shell">
        +
      </button>
    </div>
  );
}
