import { useCallback, useEffect, useState } from "react";

import { analytics as fetchAnalytics, type AnalyticsRow, type GroupBy } from "./api";

const GROUPS: GroupBy[] = ["agent", "model", "team", "project"];

/** A ratio nobody has earned yet is a dash, not a zero. */
function pct(value: number | null): string {
  return value === null ? "-" : `${Math.round(value * 100)}%`;
}

function num(value: number | null, digits = 1): string {
  return value === null ? "-" : value.toFixed(digits);
}

/** Thousands, because the interesting numbers here are all six figures. */
function tokens(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
  return String(value);
}

function duration(seconds: number | null): string {
  if (seconds === null) return "-";
  if (seconds >= 3_600) return `${(seconds / 3_600).toFixed(1)}h`;
  if (seconds >= 60) return `${Math.round(seconds / 60)}m`;
  return `${Math.round(seconds)}s`;
}

/**
 * Which agent-model pairing is actually earning its seat.
 *
 * There are no costs on this page and that is deliberate: every provider is a flat
 * subscription (D8), so the scarce resource is rate limit rather than money. The columns
 * are chosen to answer two questions - what does this pairing land, and what does it cost
 * in context to land it.
 */
/// Global by construction (D18): "which pairing earns its seat" is not a question about
/// one repository, and the `project` grouping already answers the per-project version
/// without a filter somewhere else changing what the page means.
export function Analytics({ tick }: { tick: number }) {
  const [by, setBy] = useState<GroupBy>("agent");
  const [rows, setRows] = useState<AnalyticsRow[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setRows(await fetchAnalytics(by, null));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [by]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  if (problem !== null) return <p className="error">{problem}</p>;
  if (rows === null) return <p className="empty">Counting…</p>;

  return (
    <div className="analytics">
      <div className="main__header">
        <h2>Analytics</h2>
        <div className="analytics__by">
          {GROUPS.map((option) => (
            <button
              type="button"
              key={option}
              className="nav-item"
              aria-current={by === option}
              onClick={() => setBy(option)}
            >
              <span>{option}</span>
            </button>
          ))}
        </div>
      </div>

      {rows.length === 0 ? (
        <p className="empty">No finished runs yet - there is nothing to compare.</p>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th scope="col">{by}</th>
              <th scope="col" title="Of the attempts that reached a verdict">
                accepted
              </th>
              <th scope="col" title="Attempts per slice landed. 1.0 is first-time-right">
                rework
              </th>
              <th scope="col" title="Gates that passed, of gates run">
                gates
              </th>
              <th scope="col" title="First attempt at a slice to the accepted one">
                cycle
              </th>
              <th scope="col" title="Every input token sent, cached or not - the rate-limit number">
                input
              </th>
              <th scope="col" title="Input tokens per slice landed">
                per change
              </th>
              <th scope="col" title="How much of the context sent was served from cache">
                cache hit
              </th>
              <th scope="col" title="Output tokens per thousand input">
                yield
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.group}>
                <th scope="row">{row.group}</th>
                <td>
                  {pct(row.accepted_rate)}
                  <span className="faint"> {row.accepted}/{row.accepted + row.rejected}</span>
                </td>
                <td>{num(row.rework, 2)}</td>
                <td>{pct(row.gate_pass_rate)}</td>
                <td>{duration(row.cycle_time)}</td>
                <td>{tokens(row.total_input)}</td>
                <td>{row.input_per_accepted === null ? "-" : tokens(row.input_per_accepted)}</td>
                {/* The cold-node signal: a seat that runs once per run pays to write its
                    prefix and never reads it back. */}
                <td data-cold={row.cache_hit_rate !== null && row.cache_hit_rate < 0.25}>
                  {pct(row.cache_hit_rate)}
                </td>
                <td>{num(row.yield_per_k)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
