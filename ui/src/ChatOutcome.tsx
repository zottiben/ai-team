import type { RunEvent } from "./api";
import type { ChatDetail } from "./chat-api";

export type Outcome = {
  title: string;
  description: string;
  tone: "finished" | "attention";
  action: "review" | "work" | "reply";
  button: string;
  response?: number;
};

// Presentation only, never an execution/approval state. A plainly phrased final
// question gets a useful label; quotes/code/headings must not manufacture a request.
function endsWithQuestion(message: string): boolean {
  const paragraph = message.trim().split(/\n\s*\n/).at(-1) ?? "";
  return !/^(?:>|#{1,6}\s|```|~~~)/m.test(paragraph) && /[?？][*_]*$/.test(paragraph);
}

export function chatOutcome(detail: ChatDetail | null, events: RunEvent[]): Outcome | null {
  if (!detail || detail.archived) return null;
  if (detail.state === "awaiting_approval") return {
    title: "Your approval is needed", description: "Review the plan and exact slices in Work. Nothing builds until you approve.",
    tone: "attention", action: "work", button: "Review plan",
  };
  if (["failed", "stopped", "interrupted", "team_blocked", "team_interrupted"].includes(detail.state)) return {
    title: detail.state === "stopped" ? "Turn stopped" : "Needs your attention",
    description: "Read the turn details above before continuing. Your history and working files are kept.",
    tone: "attention", action: detail.mode === "team" ? "work" : "reply", button: detail.mode === "team" ? "Open team controls" : "Write a reply",
  };
  const turn = detail.turns.at(-1);
  if (detail.state !== "idle" || detail.active_node_id !== null || turn?.node.status !== "done") return null;
  const response = [...events].reverse().find(event => event.node_run_id === turn.node.id && event.actor !== null && event.actor !== "human" && ["note", "message"].includes(event.kind) && event.message !== null);
  if (response?.message && endsWithQuestion(response.message)) return {
    title: "Question for you", description: "Read the agent's final question above. Reply when you're ready; nothing is sent automatically.",
    tone: "attention", action: "reply", button: "Reply to agent", response: response.id,
  };
  return {
    title: "Turn complete", description: "Read the result above, then review the changes or reply. Completion is not verification.",
    tone: "finished", action: "review", button: "Review changes", response: response?.id,
  };
}

export function ChatOutcome({ outcome, onAction }: { outcome: Outcome; onAction: (action: Outcome["action"]) => void }) {
  return <section className="chat-outcome" data-tone={outcome.tone} role="status" aria-label="Turn outcome">
    <span className="chat-outcome__icon" aria-hidden="true">{outcome.tone === "finished" ? "✓" : "!"}</span>
    <div><strong>{outcome.title}</strong><p>{outcome.description}</p></div>
    <button type="button" className="button" onClick={() => onAction(outcome.action)}>{outcome.button}</button>
  </section>;
}
