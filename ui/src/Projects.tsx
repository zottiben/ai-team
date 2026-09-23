import { useCallback, useEffect, useState } from "react";

import { Browse } from "./Browse";
import {
  attachRepo,
  doctor,
  projects as fetchProjects,
  registerProject,
  type Check,
  type Project,
} from "./api";

/**
 * The projects ai-team knows, and what each one is missing.
 *
 * Registering is `ait init`, done from the window and through the same function, so the two
 * cannot disagree about what a project is. A directory that is not a git repository is a
 * project of a different kind rather than an error - a triage session across four services
 * is a project too.
 *
 * The per-project health comes from the readiness report rather than being worked out
 * again here: a checkout that has moved is worth saying on the project, not discovering
 * when a run fails to lease it.
 */
export function Projects({ onChanged }: { onChanged: () => void }) {
  const [list, setList] = useState<Project[] | null>(null);
  const [checks, setChecks] = useState<Check[]>([]);
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [browsing, setBrowsing] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      // Projects may not be readable yet - a machine with no database has none - and that
      // is not worth an error on a page whose whole purpose is adding the first one.
      setList(await fetchProjects().catch(() => []));
      setChecks((await doctor()).checks);
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const add = async (event: React.FormEvent) => {
    event.preventDefault();
    if (path.trim() === "") return;
    try {
      const done = await registerProject({
        path: path.trim(),
        ...(name.trim() === "" ? {} : { name: name.trim() }),
      });
      setNote(
        done.created
          ? `Added ${done.project.name}${done.seeded_team ? " with a team of six" : ""}.`
          : `${done.project.name} was already registered.`,
      );
      setPath("");
      setName("");
      setProblem(null);
      await load();
      onChanged();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
      setNote(null);
    }
  };

  const healthOf = (slug: string) => checks.find((check) => check.id === `project.${slug}`);

  return (
    <div className="projects">
      <div className="main__header">
        <h2>Projects</h2>
      </div>

      <form className="projects__add" onSubmit={(event) => void add(event)}>
        <input
          aria-label="path"
          placeholder="~/src/your-repo"
          value={path}
          onChange={(event) => setPath(event.target.value)}
        />
        <input
          aria-label="name"
          placeholder="name (optional)"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <button
          type="button"
          className="button"
          aria-expanded={browsing}
          onClick={() => setBrowsing(!browsing)}
        >
          {browsing ? "Close" : "Browse…"}
        </button>
        <button type="submit" className="button button--primary">
          Add
        </button>
      </form>
      {browsing && (
        <Browse
          onPick={(picked) => {
            setPath(picked);
            setBrowsing(false);
          }}
        />
      )}
      {/* Relative is the one thing it cannot be: that would resolve against wherever the
          server was started, which is not where the person typing is looking. `~` is fine
          - the server expands it, because a window has no shell in front of it. */}
      <p className="faint">
        The path to a repository, starting with <code className="mono">/</code> or{" "}
        <code className="mono">~</code>. Point at anywhere inside it and ai-team registers the
        repository root.
      </p>

      {problem !== null && <p className="error">{problem}</p>}
      {note !== null && <p className="notice">{note}</p>}

      {list === null && <p className="empty">Reading…</p>}
      {list?.length === 0 && (
        <p className="empty">No projects yet. Add the repository you want to work on.</p>
      )}

      <div className="list">
        {list?.map((project) => {
          const health = healthOf(project.slug);
          return (
            <div key={project.id} className="card">
              <div className="card__row">
                <span>{project.name}</span>
                <span className="faint mono">{project.kind}</span>
                {health !== undefined && health.severity !== "fine" && (
                  <span className="status" data-status="failed">
                    {health.severity}
                  </span>
                )}
              </div>
              <span className="faint mono">{health?.detail ?? project.slug}</span>
              {health !== undefined && health.severity !== "fine" && "what" in health.fix && (
                <span className="faint">{health.fix.what}</span>
              )}
              <AttachRepo id={project.id} onDone={load} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

/**
 * Another checkout on the same project.
 *
 * A project is a container, so more than one repo is normal - a change spanning a service
 * and its client is one piece of work.
 */
function AttachRepo({ id, onDone }: { id: number; onDone: () => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [path, setPath] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  if (!open) {
    return (
      <button type="button" className="button" onClick={() => setOpen(true)}>
        Attach another repo
      </button>
    );
  }

  return (
    <form
      className="projects__add"
      onSubmit={(event) => {
        event.preventDefault();
        if (path.trim() === "") return;
        void attachRepo(id, path.trim())
          .then(() => {
            setPath("");
            setOpen(false);
            setProblem(null);
            return onDone();
          })
          .catch((error: unknown) => {
            setProblem(error instanceof Error ? error.message : String(error));
          });
      }}
    >
      <input
        aria-label={`repo path for ${id}`}
        placeholder="/Users/you/Developer/the-other-one"
        value={path}
        onChange={(event) => setPath(event.target.value)}
      />
      <button type="submit" className="button">
        Attach
      </button>
      {problem !== null && <span className="error">{problem}</span>}
    </form>
  );
}
