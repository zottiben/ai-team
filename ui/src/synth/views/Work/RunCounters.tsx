import { Lcd } from "../../primitives/Lcd";
import { counters, type RunRecord } from "./runs";

function Counter({ label, value, tone }: { label: string; value: number; tone?: "done" | "cancelled" | "live" }) {
  return <Lcd className="synth-run-counter" data-tone={tone} role="group" aria-label={label}>
    <span className="synth-silk">{label}</span>
    <strong>{value}</strong>
  </Lcd>;
}

/** Runs, Done, Cancelled and Live (§9.1), and every other recorded state beside them. */
export function RunCounters({ records }: { records: readonly RunRecord[] }) {
  const totals = counters(records);
  return <div className="synth-run-counters">
    <Counter label="Runs" value={totals.runs} />
    <Counter label="Done" value={totals.done} tone="done" />
    <Counter label="Cancelled" value={totals.cancelled} tone="cancelled" />
    <Counter label="Live" value={totals.live} tone="live" />
    {totals.others.length > 0 && <Lcd className="synth-run-others" role="group" aria-label="Other recorded states">
      <span className="synth-silk">Other states</span>
      <ul>{totals.others.map(other => <li key={other.status} data-status={other.status}>
        <span>{other.label}</span><strong>{other.count}</strong>
      </li>)}</ul>
    </Lcd>}
  </div>;
}
