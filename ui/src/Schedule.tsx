import { useCallback, useEffect, useState } from "react";

import {
  projects as fetchProjects,
  type Project as ProjectSummary,
  addReminder,
  cancelReminder,
  reminders as fetchReminders,
  type Reminder,
} from "./api";

const KINDS = {
  scheduled_run: "Run",
  reminder: "Reminder",
  idea: "Idea",
} as const;

/** `30m`, `2h`, `1d` from now, as the RFC3339 the server stores. */
function inFromNow(delta: string): string | null {
  const match = /^(\d+)([smhd]?)$/.exec(delta.trim());
  if (match === null) return null;
  const value = Number(match[1]);
  // A bare number is minutes, because that is what people mean. Reading it as seconds
  // would fire a scheduled run almost immediately and look like a broken scheduler.
  const unit = match[2] === "" ? "m" : match[2];
  const seconds = { s: 1, m: 60, h: 3_600, d: 86_400 }[unit as "s" | "m" | "h" | "d"];
  return new Date(Date.now() + value * seconds * 1000).toISOString().replace(/\.\d+Z$/, "Z");
}

/**
 * The clock, and the inbox.
 *
 * Three kinds share this page because they share a table and a due date. What differs is
 * what firing means: a reminder and an idea announce themselves, a scheduled run starts
 * work. An idea needs no date at all - that is what makes it an inbox rather than another
 * queue with a deadline.
 */
/// Global, because "what is scheduled" spans projects (D18). A scheduled *run* still needs
/// one, so the form asks - which is better than inheriting whichever project happened to be
/// selected elsewhere and scheduling work in the wrong repository.
export function Schedule({ tick }: { tick: number }) {
  const [rows, setRows] = useState<Reminder[] | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [project, setProject] = useState<string>("");
  const [problem, setProblem] = useState<string | null>(null);
  const [kind, setKind] = useState<Reminder["kind"]>("idea");
  const [title, setTitle] = useState("");
  const [when, setWhen] = useState("");

  const load = useCallback(async () => {
    try {
      setRows(await fetchReminders(null));
      setProjects(await fetchProjects().catch(() => []));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, tick]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (title.trim() === "") return;
    // Only an idea may be undated; anything else would sit there never firing.
    const due = when.trim() === "" ? null : inFromNow(when);
    if (kind !== "idea" && due === null) {
      setProblem("When? Try 30m, 2h or 1d.");
      return;
    }
    try {
      // A scheduled run needs somewhere to run, and the store refuses one without it -
      // caught here so the message names the missing thing rather than echoing a constraint.
      if (kind === "scheduled_run" && project === "") {
        setProblem("Which project should it run in?");
        return;
      }
      await addReminder({
        title,
        kind,
        ...(project === "" ? {} : { project }),
        ...(due === null ? {} : { due_at: due }),
      });
      setTitle("");
      setWhen("");
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  const live = (rows ?? []).filter((row) => row.status === "pending");

  return (
    <div className="schedule">
      <div className="main__header">
        <h2>Schedule</h2>
        {problem !== null && <span className="error">{problem}</span>}
      </div>

      <form className="schedule__add" onSubmit={(event) => void submit(event)}>
        <select
          aria-label="kind"
          value={kind}
          onChange={(event) => setKind(event.target.value as Reminder["kind"])}
        >
          {Object.entries(KINDS).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
        <input
          aria-label="title"
          placeholder={kind === "idea" ? "what if…" : "what should happen"}
          value={title}
          onChange={(event) => setTitle(event.target.value)}
        />
        <input
          aria-label="when"
          placeholder={kind === "idea" ? "someday" : "30m"}
          value={when}
          onChange={(event) => setWhen(event.target.value)}
        />
        {/* Only a run needs one: a reminder to cut a release belongs to whoever is reading
            it, not to a checkout. */}
        {kind === "scheduled_run" && (
          <select
            aria-label="project"
            value={project}
            onChange={(event) => setProject(event.target.value)}
          >
            <option value="">which project?</option>
            {projects.map((entry) => (
              <option key={entry.id} value={entry.slug}>
                {entry.name}
              </option>
            ))}
          </select>
        )}
        <button type="submit" className="button button--primary">
          Add
        </button>
      </form>

      {/* Said once, here, because a scheduled run that never fires looks like a bug in
          the scheduler rather than a process that is not running. */}
      <p className="faint">
        The clock runs in this window and in <span className="mono">ait daemon</span> - one of
        them needs to be up.
      </p>

      {rows === null ? (
        <p className="empty">Reading…</p>
      ) : live.length === 0 ? (
        <p className="empty">Nothing scheduled, and no ideas yet.</p>
      ) : (
        <div className="list">
          {live.map((row) => (
            <div key={row.id} className="card">
              <div className="card__row">
                <span className="status" data-status={row.kind === "scheduled_run" ? "queued" : "done"}>
                  {KINDS[row.kind]}
                </span>
                <span className="faint mono">{row.due_at ?? "someday"}</span>
              </div>
              <span>{row.title}</span>
              <div className="card__row">
                {row.recur !== null && <span className="faint">every {row.recur}</span>}
                <button
                  type="button"
                  className="button"
                  onClick={() => void cancelReminder(row.id).then(load)}
                >
                  Cancel
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
