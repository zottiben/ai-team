import { useCallback, useEffect, useState } from "react";

import {
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
export function Schedule({ project, tick }: { project: string | null; tick: number }) {
  const [rows, setRows] = useState<Reminder[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [kind, setKind] = useState<Reminder["kind"]>("idea");
  const [title, setTitle] = useState("");
  const [when, setWhen] = useState("");

  const load = useCallback(async () => {
    try {
      setRows(await fetchReminders(project));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project]);

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
      await addReminder({
        title,
        kind,
        ...(project === null ? {} : { project }),
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
