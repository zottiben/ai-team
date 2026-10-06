import { useCallback, useEffect, useState } from "react";

import {
  projects as fetchProjects,
  type Project as ProjectSummary,
  addReminder,
  cancelReminder,
  chatSchedules as fetchChatSchedules,
  reminders as fetchReminders,
  type ChatSchedule,
  type Reminder,
  type ScheduleOccurrence,
} from "./api";
import { chats as fetchChats, type Chat } from "./chat-api";

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

/** What one occurrence should read as. A schedule that did nothing still has to say so. */
function outcomeText(occurrence: ScheduleOccurrence): string {
  const missed =
    occurrence.skipped > 0
      ? ` ${occurrence.skipped} earlier occurrence${occurrence.skipped === 1 ? "" : "s"} were missed while the clock was down.`
      : "";
  return `${occurrence.detail ?? occurrence.outcome}${missed}`;
}

/**
 * The clock, and the inbox.
 *
 * Three kinds share this page because they share a table and a due date. What differs is
 * what firing means: a reminder and an idea announce themselves, a scheduled run starts
 * work. An idea needs no date at all - that is what makes it an inbox rather than another
 * queue with a deadline.
 */
/// Global, because "what is scheduled" spans projects (D18). Scheduled *work* names the
/// exact chat it happens in, so nothing about where it lands is inferred when it fires -
/// not the checkout, not the model, and not which conversation reads the result.
export function Schedule({
  tick,
  onOpenChat,
}: {
  tick: number;
  onOpenChat?: (project: string, chat: number) => void;
}) {
  const [rows, setRows] = useState<Reminder[] | null>(null);
  const [schedules, setSchedules] = useState<ChatSchedule[]>([]);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [project, setProject] = useState<string>("");
  const [conversations, setConversations] = useState<Chat[]>([]);
  const [chat, setChat] = useState<string>("");
  const [problem, setProblem] = useState<string | null>(null);
  const [kind, setKind] = useState<Reminder["kind"]>("idea");
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [when, setWhen] = useState("");

  const load = useCallback(async () => {
    try {
      const [reminders, linked, found] = await Promise.all([fetchReminders(null), fetchChatSchedules(), fetchProjects()]);
      setRows(reminders);
      setSchedules(linked);
      setProjects(found);
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, tick]);

  // The chats of the chosen project only: scheduling into a conversation in another
  // repository is the mistake this picker exists to make impossible.
  useEffect(() => {
    if (project === "") {
      setConversations([]);
      return;
    }
    let current = true;
    void fetchChats(project)
      .then((found) => {
        if (!current) return;
        const live = found.filter((entry) => !entry.archived);
        setConversations(live);
        setChat((id) => (live.some((entry) => String(entry.id) === id) ? id : ""));
      })
      .catch((error: unknown) => {
        if (current) setProblem(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, [project, tick]);

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
      // Scheduled work needs a conversation to happen in, and something to say in it.
      // Both are asked for here rather than resolved when it fires with nobody looking.
      if (kind === "scheduled_run" && project === "") {
        setProblem("Which project should it run in?");
        return;
      }
      if (kind === "scheduled_run" && chat === "") {
        setProblem(
          conversations.length === 0
            ? "This project has no chat yet. Start one, then schedule work in it."
            : "Which chat should it run in?",
        );
        return;
      }
      if (kind === "scheduled_run" && prompt.trim() === "") {
        setProblem("What should it send? A scheduled chat needs its own prompt.");
        return;
      }
      await addReminder({
        title,
        kind,
        ...(project === "" ? {} : { project }),
        ...(kind === "scheduled_run" ? { chat: Number(chat), prompt } : {}),
        ...(due === null ? {} : { due_at: due }),
      });
      setTitle("");
      setWhen("");
      setPrompt("");
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  // A one-shot result must not vanish the instant the clock fires it.
  const visible = (rows ?? []).filter((row) => row.status === "pending" || schedules.some(s => s.reminder_id === row.id));
  const scheduleOf = (reminder: number) =>
    schedules.find((entry) => entry.reminder_id === reminder);

  return (
    <div className="schedule">
      <div className="main__header">
        <h2>Schedule</h2>
        {problem !== null && <span className="error" role="alert">{problem}</span>}
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
        {/* Only work needs these: a reminder to cut a release belongs to whoever is
            reading it, not to a checkout or a conversation. */}
        {kind === "scheduled_run" && (
          <>
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
            <select
              aria-label="chat"
              value={chat}
              onChange={(event) => setChat(event.target.value)}
              disabled={project === ""}
            >
              <option value="">which chat?</option>
              {conversations.map((entry) => (
                <option key={entry.id} value={String(entry.id)}>
                  {entry.title}
                </option>
              ))}
            </select>
            <input
              aria-label="prompt"
              placeholder="what to send the chat"
              value={prompt}
              onChange={(event) => setPrompt(event.target.value)}
            />
          </>
        )}
        <button type="submit" className="button button--primary">
          Add
        </button>
      </form>

      {/* Said once, here, because a scheduled run that never fires looks like a bug in
          the scheduler rather than a process that is not running. */}
      <p className="faint">
        The clock runs in this window and in <span className="mono">ait daemon</span> - one of
        them needs to be up. Scheduled work runs in its own chat, and a chat that is already
        working is skipped rather than queued.
      </p>

      {rows === null ? (
        <p className="empty">Reading…</p>
      ) : visible.length === 0 ? (
        <p className="empty">Nothing scheduled, and no ideas yet.</p>
      ) : (
        <div className="list">
          {visible.map((row) => {
            const schedule = scheduleOf(row.id);
            const history = schedule?.occurrences ?? [];
            return (
              <div key={row.id} className="card">
                <div className="card__row">
                  <span
                    className="status"
                    data-status={row.kind === "scheduled_run" ? "queued" : "done"}
                  >
                    {KINDS[row.kind]}
                  </span>
                  <span className="faint mono">{row.due_at ?? "someday"}</span>
                </div>
                <span>{row.title}</span>
                {schedule && (
                  <>
                    <span className="faint">{schedule.prompt}</span>
                    <div className="card__row">
                      <button
                        type="button"
                        className="button"
                        onClick={() => onOpenChat?.(schedule.project_slug, schedule.chat_id)}
                      >
                        Open {schedule.chat_title}
                      </button>
                      <span className="faint mono">
                        {schedule.model}
                        {schedule.mode === "team" ? " · plans only, you approve builds" : ""}
                      </span>
                    </div>
                  </>
                )}
                <div className="card__row">
                  {row.recur !== null && <span className="faint">every {row.recur}</span>}
                  {row.status === "pending" ? <button
                    type="button"
                    className="button"
                    onClick={() => void cancelReminder(row.id).then(load).catch(e => setProblem(String(e)))}
                  >
                    Cancel
                  </button> : <span className="faint">{row.status} · occurrence history kept</span>}
                </div>
                {history.length > 0 && (
                  <ul className="schedule__history">
                    {history.map((occurrence) => (
                      <li key={occurrence.id}>
                        <span className="status" data-status={statusOf(occurrence.outcome)}>
                          {occurrence.outcome}
                        </span>
                        <span className="faint mono">{occurrence.occurrence_at}</span>
                        <span>{outcomeText(occurrence)}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/** Semantic status tokens only: a colour here would not follow the theme. */
function statusOf(outcome: ScheduleOccurrence["outcome"]): string {
  if (outcome === "started") return "running";
  if (outcome === "claimed") return "queued";
  return "blocked";
}
