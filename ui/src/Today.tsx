import { useCallback, useEffect, useMemo, useState } from "react";

import {
  analytics as fetchAnalytics,
  projects as fetchProjects,
  runs as fetchRuns,
  today as fetchToday,
  type AnalyticsRow,
  type Project,
  type Run,
  type TodayItem,
} from "./api";

/** What each tier means, said once, where a reader can see it. */
const URGENCY: Record<
  TodayItem["urgency"],
  { label: string; why: string; status: string }
> = {
  blocking: { label: "Blocking", why: "a run is parked and you are the reason", status: "parked" },
  overdue: { label: "Overdue", why: "its time has already passed", status: "failed" },
  failed: { label: "Failed", why: "it will not un-fail on its own", status: "failed" },
  review: { label: "Review", why: "finished work waiting to be looked at", status: "done" },
  question: { label: "Question", why: "the plan is waiting on an answer", status: "blocked" },
  due: { label: "Due", why: "due about now", status: "queued" },
  in_flight: { label: "In flight", why: "under way; nothing for you to do", status: "running" },
};

const URGENCY_ORDER = Object.keys(URGENCY) as TodayItem["urgency"][];

type Operations = {
  items: TodayItem[];
  projects: Project[];
  runs: Run[];
  analytics: AnalyticsRow[];
  updatedAt: Date;
};

/**
 * Today: one live operating picture across every project.
 *
 * The queue is rendered in exactly the order core returned it. Counters, bars, and project
 * summaries are different views over that same answer; none of them feed a second ranking
 * back into the queue.
 */
export function Today({
  tick,
  onOpenRun,
  onOpenReview,
}: {
  tick: number;
  onOpenRun: (id: number, project: string | null) => void;
  /** A review waiting on you opens as itself, in the checkout its run belongs to. */
  onOpenReview?: (id: number, project: string | null, run: number | null) => void;
}) {
  const [operations, setOperations] = useState<Operations | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      // Today is authoritative. The supporting panels are useful context, but one stale
      // analytics query must not hide the thing waiting on a person right now.
      const [items, projects, runs, analytics] = await Promise.all([
        fetchToday(),
        fetchProjects().catch(() => []),
        fetchRuns().catch(() => []),
        fetchAnalytics("project", null).catch(() => []),
      ]);
      setOperations({ items, projects, runs, analytics, updatedAt: new Date() });
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, tick]);

  if (problem !== null) return <p className="error">{problem}</p>;
  if (operations === null) return <p className="empty">Building today’s operating picture…</p>;

  const { items, projects, runs, analytics, updatedAt } = operations;
  const top = items[0];
  const needsYou = items.filter((item) => item.urgency !== "in_flight").length;
  const inFlight = items.filter((item) => item.urgency === "in_flight").length;
  const reviews = items.filter((item) => item.urgency === "review").length;
  const activeRuns = runs.filter((run) =>
    ["queued", "planning", "running", "blocked"].includes(run.status),
  ).length;

  return (
    <div className="today">
      <div className="today__masthead">
        <div>
          <span className="eyebrow">Personal operations</span>
          <h2>Today</h2>
          <p className="faint">One ranked queue across every repository and every seat.</p>
        </div>
        <span className="today__live">
          <span className="today__live-dot" aria-hidden="true" />
          Live · {updatedAt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
        </span>
      </div>

      <section className="today__pulse" aria-label="Operations pulse">
        <Metric value={needsYou} label="Needs you" tone={needsYou > 0 ? "attention" : "quiet"} />
        <Metric value={inFlight} label="Agents working" tone={inFlight > 0 ? "live" : "quiet"} />
        <Metric value={reviews} label="Ready to review" tone={reviews > 0 ? "review" : "quiet"} />
        <Metric value={activeRuns} label="Active runs" tone={activeRuns > 0 ? "live" : "quiet"} />
      </section>

      <div className="today__layout">
        <div className="today__primary">
          {top === undefined ? (
            <section className="today__clear">
              <span className="today__clear-mark" aria-hidden="true">✓</span>
              <div>
                <h3>Nothing is waiting on you</h3>
                <p className="faint">The board is where work starts. Active work will appear here.</p>
              </div>
            </section>
          ) : (
            <section className="today__first">
              <div className="today__section-heading">
                <div>
                  <span className="eyebrow">Do this first</span>
                  <h3>{URGENCY[top.urgency].why}</h3>
                </div>
                <span className="today__rank">#1</span>
              </div>
              <Row item={top} onOpenRun={onOpenRun} onOpenReview={onOpenReview} priority />
            </section>
          )}

          {items.length > 1 && (
            <section className="today__queue">
              <div className="today__section-heading">
                <div>
                  <span className="eyebrow">Ranked by urgency</span>
                  <h3>Next in line</h3>
                </div>
                <span className="faint">{items.length - 1} more</span>
              </div>
              <div className="list">
                {items.slice(1).map((item, index) => (
                  <Row
                    key={`${item.kind}-${item.title}-${index}`}
                    item={item}
                    rank={index + 2}
                    onOpenRun={onOpenRun}
                    onOpenReview={onOpenReview}
                  />
                ))}
              </div>
            </section>
          )}
        </div>

        <aside className="today__rail">
          <Workload items={items} />
          <Projects projects={projects} items={items} />
          <RecentRuns runs={runs} projects={projects} onOpenRun={onOpenRun} />
          <Throughput rows={analytics} />
        </aside>
      </div>
    </div>
  );
}

function Metric({ value, label, tone }: { value: number; label: string; tone: string }) {
  return (
    <div className="today-metric" data-tone={tone}>
      <strong>{value}</strong>
      <span>{label}</span>
    </div>
  );
}

function Workload({ items }: { items: TodayItem[] }) {
  const counts = useMemo(
    () =>
      URGENCY_ORDER.map((urgency) => ({
        urgency,
        count: items.filter((item) => item.urgency === urgency).length,
      })).filter((entry) => entry.count > 0),
    [items],
  );
  const largest = Math.max(1, ...counts.map((entry) => entry.count));

  return (
    <section className="today-panel">
      <div className="today__section-heading">
        <div>
          <span className="eyebrow">Queue shape</span>
          <h3>Workload</h3>
        </div>
        <span className="faint">{items.length} total</span>
      </div>
      {counts.length === 0 ? (
        <p className="faint">No queued work.</p>
      ) : (
        <div className="today-bars">
          {counts.map(({ urgency, count }) => (
            <div className="today-bar" key={urgency}>
              <div className="today-bar__label">
                <span>{URGENCY[urgency].label}</span>
                <strong>{count}</strong>
              </div>
              <span className="today-bar__track" aria-hidden="true">
                <span
                  data-status={URGENCY[urgency].status}
                  style={{ width: `${Math.max(12, (count / largest) * 100)}%` }}
                />
              </span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function Projects({ projects, items }: { projects: Project[]; items: TodayItem[] }) {
  return (
    <section className="today-panel">
      <div className="today__section-heading">
        <div>
          <span className="eyebrow">Across the desk</span>
          <h3>Repositories</h3>
        </div>
        <span className="faint">{projects.length}</span>
      </div>
      {projects.length === 0 ? (
        <p className="faint">No repositories yet.</p>
      ) : (
        <div className="today-projects">
          {projects.map((project) => {
            const waiting = items.filter(
              (item) => item.project === project.slug && item.urgency !== "in_flight",
            ).length;
            const moving = items.filter(
              (item) => item.project === project.slug && item.urgency === "in_flight",
            ).length;
            return (
              <div className="today-project" key={project.id}>
                <span className="workspace-dot" data-status={moving > 0 ? "running" : "available"} />
                <div>
                  <strong>{project.name}</strong>
                  {project.name.toLowerCase() !== project.slug && (
                    <span className="faint mono">{project.slug}</span>
                  )}
                </div>
                <span className="today-project__counts">
                  {waiting > 0 ? `${waiting} waiting` : moving > 0 ? `${moving} working` : "clear"}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function RecentRuns({
  runs,
  projects,
  onOpenRun,
}: {
  runs: Run[];
  projects: Project[];
  onOpenRun: (id: number, project: string | null) => void;
}) {
  return (
    <section className="today-panel">
      <div className="today__section-heading">
        <div>
          <span className="eyebrow">Latest changes</span>
          <h3>Recent activity</h3>
        </div>
      </div>
      {runs.length === 0 ? (
        <p className="faint">No recorded runs yet.</p>
      ) : (
        <div className="today-recent">
          {runs.slice(0, 4).map((run) => {
            const project = projects.find((entry) => entry.id === run.project_id);
            return (
              <button
                type="button"
                key={run.id}
                onClick={() => onOpenRun(run.id, project?.slug ?? null)}
              >
                <span className="workspace-dot" data-status={run.status} />
                <span>
                  <strong>{run.prompt}</strong>
                  <span className="faint">
                    {project?.name ?? `run #${run.id}`} · {run.status.replaceAll("_", " ")}
                  </span>
                </span>
                <span aria-hidden="true">→</span>
              </button>
            );
          })}
        </div>
      )}
    </section>
  );
}

function Throughput({ rows }: { rows: AnalyticsRow[] }) {
  const attempts = rows.reduce((sum, row) => sum + row.attempts, 0);
  const accepted = rows.reduce((sum, row) => sum + row.accepted, 0);
  const gates = rows.reduce((sum, row) => sum + row.gates_run, 0);
  const passed = rows.reduce((sum, row) => sum + row.gates_passed, 0);
  const acceptance = attempts === 0 ? null : Math.round((accepted / attempts) * 100);
  const gateRate = gates === 0 ? null : Math.round((passed / gates) * 100);

  return (
    <section className="today-panel">
      <div className="today__section-heading">
        <div>
          <span className="eyebrow">All recorded work</span>
          <h3>Throughput</h3>
        </div>
      </div>
      <div className="today-throughput">
        <div><strong>{accepted}</strong><span>accepted</span></div>
        <div><strong>{acceptance === null ? "—" : `${acceptance}%`}</strong><span>acceptance</span></div>
        <div><strong>{gateRate === null ? "—" : `${gateRate}%`}</strong><span>gates pass</span></div>
      </div>
    </section>
  );
}

function Row({
  item,
  rank,
  priority = false,
  onOpenRun,
  onOpenReview,
}: {
  item: TodayItem;
  rank?: number;
  priority?: boolean;
  onOpenRun: (id: number, project: string | null) => void;
  onOpenReview?: (id: number, project: string | null, run: number | null) => void;
}) {
  const review = item.review_id ?? null;
  const open =
    review !== null && onOpenReview !== undefined
      ? () => onOpenReview(review, item.project, item.run_id)
      : item.run_id !== null
        ? () => onOpenRun(item.run_id ?? 0, item.project)
        : null;
  const tier = URGENCY[item.urgency];
  const body = (
    <>
      {rank !== undefined && <span className="today-row__rank">{rank}</span>}
      <div className="today-row__body">
        <div className="card__row">
          <span className="status" data-status={tier.status}>{tier.label}</span>
          <span className="faint mono">{item.project ?? "personal"}</span>
          <span className="faint">{item.kind.replaceAll("_", " ")}</span>
        </div>
        <strong>{item.title}</strong>
        {item.detail !== null && <span className="faint">{item.detail}</span>}
      </div>
      {open !== null && <span className="today-row__open" aria-hidden="true">→</span>}
    </>
  );
  const className = priority ? "card today-row today-row--priority" : "card today-row";

  return open === null ? (
    <div className={className}>{body}</div>
  ) : (
    <button type="button" className={className} onClick={open}>
      {body}
    </button>
  );
}
