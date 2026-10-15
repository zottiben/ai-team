import { useRef, useState } from "react";
import { SelectionToolbar } from "./SelectionToolbar";
import { Led } from "../../primitives/Led";
import { Silk } from "../../primitives/Silk";
import type { AgentCommand } from "./agent-shell";

/**
 * The agent's shell, read-only (§12.1).
 *
 * No session id, no input handler and no write route: this renders recorded activity,
 * so there is nothing here that could carry a keystroke to the agent's process. What it
 * shows is bounded by what the host kept, and every bound is on the page - a sampled
 * tail says it is a sample, a truncated one says it was cut.
 */
export function AgentMirror({ commands, effects, onSend }: { commands: readonly AgentCommand[]; effects: boolean; onSend?: (text: string) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useState<{ text: string; top: number; left: number } | null>(null);
  return (
    <div ref={host} className="synth-screen" data-effects={effects || undefined}>
      <div className="synth-screen__well synth-mirror" aria-label="Agent shell, read only">
        <p className="synth-mirror__note">
          <span aria-hidden="true">🔒</span> Read-only. This is the agent's recorded shell activity - there is no input
          to it from here. Output is bounded: in-flight text is a sampled tail and can miss what happened between
          samples.
        </p>
        {commands.length === 0 ? (
          <p className="synth-muted">No captured shell evidence is available in this chat.</p>
        ) : (
          <ol className="synth-mirror__list">
            {commands.map((command) => (
              <li key={command.id}>
                <div className="synth-mirror__heading">
                  <Led
                    state="on"
                    signal={command.state === "returned" ? "done" : "attention"}
                    size={7}
                  />
                  <code>{command.command ?? "(command not recorded)"}</code>
                  {command.commandTruncated && <span className="synth-muted">truncated</span>}
                </div>
                <p className="synth-mirror__state">
                  {command.state === "returned"
                      ? "Returned"
                      : command.state === "failed"
                        ? "Failed"
                        : "Outcome not recorded"}
                  {command.durationMs !== null && ` · ${Math.round(command.durationMs)}ms`}
                  {command.sampled && " · sampled tail, may omit output"}
                  {command.outputTruncated && " · output truncated"}
                </p>
                <small>Node {command.nodeId ?? "unknown"} · session {command.sessionId ?? "unknown"} · {command.correlation.replaceAll("_", " ")}</small>
                {command.output !== null && <textarea className="synth-mirror__output" readOnly
                  aria-label={`Recorded shell output ${command.id}`} value={command.output}
                  rows={Math.min(16, Math.max(3, command.output.split("\n").length))}
                  onSelect={event => {
                    const area = event.currentTarget;
                    const text = area.value.slice(area.selectionStart, area.selectionEnd);
                    if (!text || !host.current) { setSelection(null); return; }
                    const rect = area.getBoundingClientRect(), parent = host.current.getBoundingClientRect();
                    setSelection({ text, top: Math.max(0, Math.min(rect.bottom, window.innerHeight - 44) - parent.top), left: Math.max(0, rect.left - parent.left) });
                  }} />}
              </li>
            ))}
          </ol>
        )}
      </div>
      {selection && <SelectionToolbar {...selection} onSend={onSend} />}
      <Silk className="synth-mirror__footer">chat shell evidence · newest 20 entries · read-only mirror</Silk>
    </div>
  );
}
