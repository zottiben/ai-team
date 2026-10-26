import type { TerminalCommand, TerminalDeck } from "../../../api";
import { Lcd } from "../../primitives/Lcd";
import { Led } from "../../primitives/Led";
import { Plate } from "../../primitives/Plate";
import { Scope } from "../../primitives/Scope";
import type { Series } from "../../primitives/graph";
import type { AgentCommand } from "./agent-shell";
import type { Renderer } from "./Screen";

/**
 * What has been run here, and how it ended (§12.1).
 *
 * Every number on this rack comes from a receipt: an exit status read from the process
 * that was the command, a duration measured between its start and its end. A command
 * whose outcome nobody observed says so. The one measurement that is the *window's*
 * rather than the process's - the output rate - is labelled as such, because a window
 * that was closed for a minute did not see that minute.
 */
/** Stable, so the scope reads its snapshot through `read` rather than re-running on
 *  a fresh array every render. Collection lives in the sampler (§13.1). */
const NO_SAMPLES: Series = [];

export function ProcessRack({
  deck,
  rate,
  active,
  agent,
  renderer,
  dropped,
}: {
  deck: TerminalDeck | null;
  rate: () => Series;
  active: boolean;
  agent: readonly AgentCommand[];
  renderer: Renderer | null;
  dropped: number;
}) {
  const running = deck?.running ?? [];
  const history = deck?.history ?? [];
  const last: TerminalCommand | undefined = history[0];
  const live = running[0];
  const lastAgent = agent.at(-1);

  return (
    <aside className="synth-rack" aria-label="Process rack">
      <Plate tone="night" heading="Last command">
        {live ? (
          <>
            <Lcd className="synth-rack__command">
              <code>{live.command}</code>
            </Lcd>
            <dl className="synth-rack__minis">
              <Mini label="Exit" value="Running" />
              <Mini label="Time" value={`${Math.round(live.duration_ms / 100) / 10}s`} />
              <Mini label="Lines" value={`${live.lines}${live.lines_exact ? "" : "+"}`} />
            </dl>
          </>
        ) : last ? (
          <>
            <Lcd className="synth-rack__command">
              <code>{last.command}</code>
            </Lcd>
            <dl className="synth-rack__minis">
              <Mini
                label="Exit"
                value={last.state === "finished" ? String(last.exit_code ?? "unknown") : outcome(last)}
              />
              <Mini label="Time" value={last.duration_ms === null ? "—" : `${Math.round(last.duration_ms / 100) / 10}s`} />
              <Mini
                label="Lines"
                value={last.lines === null ? "—" : `${last.lines}${last.lines_exact ? "" : "+"}`}
              />
            </dl>
          </>
        ) : (
          <p className="synth-muted">
            No command has been run from a pad in this checkout. What you type in a shell is yours - ai-team does not
            read it back.
          </p>
        )}
        <Scope label="Output rate" unit="lines/s" samples={NO_SAMPLES} read={rate} active={active} signal="bash" />
        <p className="synth-muted">Lines this window received. It cannot see output while it is closed.</p>
        {dropped > 0 && (
          <p className="synth-muted" role="status">
            {dropped.toLocaleString()} bytes of earlier output fell out of the scrollback before this window read them.
          </p>
        )}
        {renderer && (
          <p className="synth-muted">
            Renderer: {renderer.kind === "webgl" ? "WebGL" : "standard"}
            {renderer.reason ? ` · ${renderer.reason}` : ""}
          </p>
        )}
      </Plate>

      <Plate tone="midnight" heading="History">
        {history.length === 0 ? (
          <p className="synth-muted">Nothing yet.</p>
        ) : (
          <ol className="synth-rack__history">
            {history.map((row) => (
              <li key={row.token}>
                <Led
                  state={row.state === "running" ? "pulse" : "on"}
                  signal={
                    row.state === "running"
                      ? "working"
                      : row.state === "finished" && row.exit_code === 0
                        ? "done"
                        : "attention"
                  }
                  size={6}
                />
                <code>{row.command}</code>
                <span className="synth-muted">
                  {row.state === "finished"
                    ? `${row.exit_code === null ? "no status" : `exit ${row.exit_code}`}${row.duration_ms === null ? "" : ` · ${Math.round(row.duration_ms / 100) / 10}s`}`
                    : outcome(row)}
                </span>
              </li>
            ))}
          </ol>
        )}
      </Plate>

      <Plate tone="flamingo" heading="Agent shell">
        {lastAgent ? (
          <>
            <Lcd className="synth-rack__command">
              <code>{lastAgent.command ?? "(command not recorded)"}</code>
            </Lcd>
            <p className="synth-muted">
              {lastAgent.state === "returned" ? "Returned" : lastAgent.state === "failed" ? "Failed" : "Outcome not recorded"}
              {lastAgent.durationMs !== null && ` · ${Math.round(lastAgent.durationMs)}ms`} · read-only
            </p>
          </>
        ) : (
          <p className="synth-muted">The agent has not run a shell command in this chat.</p>
        )}
      </Plate>
    </aside>
  );
}

/** What to say about a row with no observed outcome. Never a pass, never still running. */
function outcome(row: TerminalCommand): string {
  return row.state === "running" ? "Running" : "Outcome not recorded";
}

function Mini({ label, value }: { label: string; value: string }) {
  return (
    <div className="synth-rack__mini">
      <dt className="synth-silk">{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
