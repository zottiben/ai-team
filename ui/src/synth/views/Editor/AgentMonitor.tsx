import type { SeatTouch } from "../../../chat-overview-api";
import { HwButton } from "../../primitives/HwButton";
import { Lcd } from "../../primitives/Lcd";
import { Led } from "../../primitives/Led";
import { overlapping, type Presence } from "./presence";

const LANES = ["read", "edit", "bash", "mcp", "other"] as const;
type Lane = (typeof LANES)[number];
const STEPS = 24;

/** Which lane a tool belongs to. Unknown tools are "other", never guessed into a lane. */
export function laneOf(tool: string): Lane {
  const name = tool.toLowerCase();
  if (["read", "grep", "glob", "search", "ls"].includes(name)) return "read";
  if (["edit", "write", "apply_patch", "multiedit"].includes(name)) return "edit";
  if (["bash", "powershell", "shell"].includes(name)) return "bash";
  if (name.includes("__") || name.startsWith("mcp")) return "mcp";
  return "other";
}

/**
 * The mini pattern: the last 24 touches, each in its tool's lane.
 *
 * Real steps in real order, not an animation. An empty column is a step that did not
 * happen rather than a gap in the data.
 */
export function pattern(touches: SeatTouch[]): Record<Lane, boolean[]> {
  const recent = [...touches].sort((a, b) => a.at.localeCompare(b.at)).slice(-STEPS);
  const lanes = Object.fromEntries(
    LANES.map(lane => [lane, Array.from({ length: STEPS }, () => false)]),
  ) as Record<Lane, boolean[]>;
  const offset = STEPS - recent.length;
  recent.forEach((touch, index) => {
    const lane = lanes[laneOf(touch.tool)];
    if (lane !== undefined) lane[offset + index] = true;
  });
  return lanes;
}

/** §11.4's monitor: who is working, on what, and what it has in common with you. */
export default function AgentMonitor({ presence, elapsed, open, onOpenChat, onSteer }: {
  presence: Presence;
  elapsed: number | null;
  /** Paths open in the editor, for the overlap list. */
  open: string[];
  onOpenChat?: () => void;
  onSteer?: () => void;
}) {
  const overlap = overlapping(presence, open);
  const others = presence.recent.filter(touch => !open.includes(touch.path));
  const lanes = pattern(presence.touched);

  return <aside className="synth-editor__monitor" aria-label="Agent monitor">
    <header className="synth-silk">Agent</header>

    <Lcd className="synth-editor__elapsed" variant={presence.working ? "working" : undefined}>
      <span className="synth-lcd__label">{presence.role ?? "No seat"}</span>
      <span className="synth-mono synth-editor__elapsed-value">
        {elapsed === null ? "idle" : `${elapsed} s`}
      </span>
    </Lcd>

    <div className="synth-editor__pattern" aria-hidden="true">
      {LANES.map(lane => <div className="synth-editor__lane" key={lane} data-lane={lane}>
        {(lanes[lane] ?? []).map((on, index) => <span key={index} className="synth-editor__cell" data-on={on || undefined} />)}
      </div>)}
    </div>

    <section className="synth-editor__now">
      <h3 className="synth-silk">Now</h3>
      {presence.now === null
        ? <p className="synth-muted">Nothing recorded in this checkout.</p>
        : <>
            <p className="synth-mono">{presence.now.summary}</p>
            {presence.now.detail !== null && <p className="synth-muted synth-mono">{presence.now.detail}</p>}
          </>}
    </section>

    <section className="synth-editor__overlap">
      <h3 className="synth-silk">Overlap with you</h3>
      {overlap.length === 0
        ? <p className="synth-muted">None of your open files in the last five minutes.</p>
        : <ul className="synth-editor__files">
            {overlap.map(touch => <li key={touch.path}>
              <Lcd variant="working" className="synth-editor__file">
                <Led state="pulse" signal="result" size={7} />
                <span className="synth-mono">{touch.path}</span>
                <span className="synth-muted">{touch.writes > 0 ? "edited" : "read"}</span>
              </Lcd>
            </li>)}
          </ul>}
      {others.length > 0 && <>
        <h3 className="synth-silk">Also touched</h3>
        <ul className="synth-editor__files">
          {others.slice(0, 12).map(touch => <li key={touch.path} className="synth-mono synth-muted">{touch.path}</li>)}
        </ul>
      </>}
    </section>

    <footer className="synth-editor__monitor-actions">
      {onOpenChat !== undefined && <HwButton onClick={onOpenChat}>Open chat</HwButton>}
      {onSteer !== undefined && <HwButton
        onClick={onSteer}
        title="Takes you to the conversation, where stopping and steering is confirmed"
      >Stop &amp; steer</HwButton>}
    </footer>
  </aside>;
}
