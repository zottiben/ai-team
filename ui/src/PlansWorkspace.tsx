import { useEffect, useState, type ReactNode } from "react";
import { ChatPlanning } from "./ChatPlan";
import { statusLabel } from "./PlanForms";
import type { PlanLibrary, PlanLibraryEntry } from "./plan-library-api";

/** Editors stay mounted after first opening. Filtering or selecting another plan
 * must not silently discard an unsaved section/slice or retarget its revision. */
export function PlansWorkspace({
  board,
  filters,
  tick,
  onOpenChat,
  onChanged,
}: {
  board: PlanLibrary;
  filters: ReactNode;
  tick: number;
  onOpenChat: (project: string, chat: number) => void;
  onChanged: () => void;
}) {
  const [selected, setSelected] = useState<number | null>(null);
  const [opened, setOpened] = useState<number[]>([]);
  const [known, setKnown] = useState<Record<number, PlanLibraryEntry>>({});
  const [search, setSearch] = useState("");
  useEffect(() => {
    setKnown((previous) => ({
      ...previous,
      ...Object.fromEntries(
        board.entries.map((entry) => [entry.chat_id, entry]),
      ),
    }));
    if (!board.entries.some((entry) => entry.chat_id === selected)) {
      const first = board.entries[0];
      setSelected(first?.chat_id ?? null);
      if (first) {
        setOpened((previous) =>
          previous.includes(first.chat_id)
            ? previous
            : [...previous, first.chat_id],
        );
      }
    }
  }, [board.entries, selected]);
  const choose = (id: number) => {
    setSelected(id);
    setOpened((previous) =>
      previous.includes(id) ? previous : [...previous, id],
    );
  };
  const visible = board.entries.filter((entry) =>
    `${entry.title} ${entry.project_name} ${entry.chat_title}`
      .toLowerCase()
      .includes(search.toLowerCase()),
  );
  // A removed/archived selection must disappear too, not linger in the editor.
  // Its mounted draft is retained for an explicit later selection.
  const current = board.entries.find((entry) => entry.chat_id === selected);
  return (
    <div className="plans-workspace">
      <nav className="plans-navigation" aria-label="Plans by project">
        <label className="plans-search">
          Search plans
          <input
            type="search"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Title, project or chat…"
          />
        </label>
        {filters}
        {visible.length === 0 && (
          <p className="faint">
            {board.projects.length
              ? "No plans match these filters."
              : "No visible plans. Plan inside a chat, show archived chat plans, or import a standalone plan below."}
          </p>
        )}
        {board.projects.map((project) => {
          const entries = visible.filter(
            (entry) => entry.project_id === project.id,
          );
          if (entries.length === 0) return null;
          return (
            <details className="plan-project-group" key={project.id} open>
              <summary>
                <strong>{project.name}</strong>
                <span className="faint">{entries.length}</span>
              </summary>
              <div>
                {entries.map((entry) => (
                  <button
                    className="plan-navigation-item"
                    type="button"
                    key={entry.plan_id}
                    aria-label={`Open plan ${entry.title}`}
                    aria-current={
                      entry.chat_id === selected ? "page" : undefined
                    }
                    onClick={() => choose(entry.chat_id)}
                  >
                    <strong>{entry.title}</strong>
                    <span className="faint">
                      {entry.done}/{entry.slices} · {statusLabel(entry.status)}
                      {entry.open_questions > 0 &&
                        ` · ${entry.open_questions} questions`}
                      {entry.chat_archived && " · archived"}
                    </span>
                    {entry.slices > 0 && (
                      <progress
                        max={entry.slices}
                        value={entry.done}
                        aria-label={`${entry.title} reported slice progress`}
                      />
                    )}
                  </button>
                ))}
              </div>
            </details>
          );
        })}
      </nav>
      <div className="plans-working-area">
        {current &&
          !visible.some((entry) => entry.chat_id === current.chat_id) && (
            <p className="notice">
              The selected plan is hidden by the current filters. Its editor and
              drafts are kept open.
            </p>
          )}
        {!current && (
          <p className="empty">Choose a plan to view its Board or Plan here.</p>
        )}
        {opened.map((id) => {
          const entry = known[id];
          if (!entry) return null;
          return (
            <section
              key={id}
              hidden={current?.chat_id !== id}
              className="plan-workspace-view"
              aria-label={`${entry.title} workspace`}
            >
              <header className="plan-workspace-context">
                <span
                  className="faint"
                  title={`${entry.project_name} · ${entry.chat_title}`}
                >
                  {entry.project_name} · {entry.chat_title}
                </span>
                <button
                  type="button"
                  className="button"
                  aria-label={`Open ${entry.chat_title}`}
                  onClick={() => onOpenChat(entry.project_slug, entry.chat_id)}
                >
                  Open chat ↗
                </button>
              </header>
              {entry.imported && (
                <p className="plan-library-provenance">
                  Imported from <code>{entry.imported.source_path}</code>
                  {entry.imported.source_plan && (
                    <> ({entry.imported.source_plan})</>
                  )}
                  . That database is not written to or kept in step with this
                  copy.
                </p>
              )}
              <ChatPlanning
                chatId={entry.chat_id}
                tick={selected === id ? tick : 0}
                archived={entry.chat_archived}
                workspace
                onChanged={onChanged}
              />
            </section>
          );
        })}
      </div>
    </div>
  );
}
