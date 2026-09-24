import { EditorView } from "@codemirror/view";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  readFile,
  saveFile,
  search as runSearch,
  tree as fetchTree,
  type Hit,
  type TreeEntry,
  type Where,
} from "./api";
import { stateFor } from "./codemirror";
import { languageServer } from "./language";

/** One open file. The saved text is kept so dirty is a comparison, not a guess. */
type Buffer = {
  path: string;
  text: string;
  saved: string;
  editable: boolean;
};

export function Editor({
  project,
  workspace = null,
  node,
}: {
  project: string | null;
  workspace?: string | null;
  node: number | null;
}) {
  const [open, setOpen] = useState<Buffer[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const where: Where | null = project === null ? null : { project, workspace, node };
  const current = open.find((buffer) => buffer.path === active) ?? null;

  const openPath = useCallback(
    async (path: string) => {
      if (where === null) return;
      setActive(path);
      // Already open: switching tabs must not discard unsaved edits by re-reading.
      if (open.some((buffer) => buffer.path === path)) return;
      try {
        const body = await readFile(where, path);
        setOpen((buffers) => [
          ...buffers,
          { path, text: body.text, saved: body.text, editable: body.editable },
        ]);
        setProblem(null);
      } catch (error: unknown) {
        setProblem(error instanceof Error ? error.message : String(error));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [project, workspace, node, open],
  );

  const save = useCallback(async () => {
    if (where === null || current === null) return;
    try {
      await saveFile(where, current.path, current.text);
      setOpen((buffers) =>
        buffers.map((buffer) =>
          buffer.path === current.path ? { ...buffer, saved: buffer.text } : buffer,
        ),
      );
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, workspace, node, current]);

  // Ctrl/Cmd-S, because that is the muscle memory and the browser's own Save is useless
  // here.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "s") {
        event.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [save]);

  if (project === null) {
    return <p className="empty">Pick a project to open its files.</p>;
  }

  return (
    <div className="editor">
      <div className="editor__side">
        <Search where={{ project, workspace, node }} onOpen={(path) => void openPath(path)} />
        <Tree where={{ project, workspace, node }} onOpen={(path) => void openPath(path)} />
      </div>

      <div className="editor__main">
        {open.length > 0 && (
          <div className="editor__tabs">
            {open.map((buffer) => (
              <button
                type="button"
                key={buffer.path}
                className="editor__tab"
                aria-current={buffer.path === active}
                onClick={() => setActive(buffer.path)}
              >
                <span>{buffer.path.slice(buffer.path.lastIndexOf("/") + 1)}</span>
                {/* A dot, not a word: it has to be readable at a glance across a row of
                    tabs, and "modified" is four times the width of the filename. */}
                {buffer.text !== buffer.saved && <span className="editor__dirty">●</span>}
                <span
                  className="editor__close"
                  role="button"
                  tabIndex={-1}
                  aria-label={`close ${buffer.path}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    setOpen((buffers) => buffers.filter((entry) => entry.path !== buffer.path));
                    // A neighbour takes the closed file's place - the next tab, else the
                    // one before - rather than an empty pane beside tabs still open.
                    if (active === buffer.path) {
                      const index = open.findIndex((entry) => entry.path === buffer.path);
                      setActive((open[index + 1] ?? open[index - 1])?.path ?? null);
                    }
                  }}
                >
                  ×
                </span>
              </button>
            ))}
            {current !== null && (
              <button
                type="button"
                className="button button--primary editor__save"
                disabled={current.text === current.saved}
                onClick={() => void save()}
              >
                Save
              </button>
            )}
          </div>
        )}

        {problem !== null && <p className="error">{problem}</p>}

        {current === null ? (
          <p className="empty">Open a file from the tree, or search for one.</p>
        ) : current.editable ? (
          <Surface
            key={current.path}
            where={{ project, workspace, node }}
            path={current.path}
            text={current.saved}
            onChange={(text) =>
              setOpen((buffers) =>
                buffers.map((buffer) =>
                  buffer.path === current.path ? { ...buffer, text } : buffer,
                ),
              )
            }
          />
        ) : (
          // Refused rather than rendered: a lossy read followed by a save would replace
          // every undecodable byte and quietly corrupt the file.
          <p className="empty">{current.path} is not text - not opening it for editing.</p>
        )}
      </div>
    </div>
  );
}

/**
 * The CodeMirror instance.
 *
 * Mounted once per path, keyed by it, because CodeMirror owns its document - swapping the
 * text underneath a live view would discard the undo history along with it.
 */
function Surface({
  where,
  path,
  text,
  onChange,
}: {
  where: Where;
  path: string;
  text: string;
  onChange: (text: string) => void;
}) {
  const host = useRef<HTMLDivElement | null>(null);
  const latest = useRef(onChange);
  latest.current = onChange;

  useEffect(() => {
    if (host.current === null) return undefined;
    const view = new EditorView({
      state: stateFor(
        path,
        text,
        (value) => latest.current(value),
        languageServer(where, path),
      ),
      parent: host.current,
    });
    return () => view.destroy();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  return <div className="editor__surface" ref={host} />;
}

function Tree({ where, onOpen }: { where: Where; onOpen: (path: string) => void }) {
  const [expanded, setExpanded] = useState<Record<string, TreeEntry[]>>({});
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(
    async (path: string) => {
      try {
        const entries = await fetchTree(where, path);
        setExpanded((state) => ({ ...state, [path]: entries }));
        setProblem(null);
      } catch (error: unknown) {
        setProblem(error instanceof Error ? error.message : String(error));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [where.project, where.node],
  );

  useEffect(() => {
    void load("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [where.project, where.node]);

  if (problem !== null) return <p className="error">{problem}</p>;

  const draw = (path: string, depth: number): React.ReactNode =>
    (expanded[path] ?? []).map((entry) => (
      <div key={entry.path}>
        <button
          type="button"
          className="tree__row"
          style={{ paddingLeft: `${depth * 0.75 + 0.5}rem` }}
          onClick={() => {
            if (!entry.dir) {
              onOpen(entry.path);
              return;
            }
            // Collapse by forgetting, expand by fetching: one level at a time, because a
            // recursive walk of a real repo is thousands of rows nobody draws.
            setExpanded((state) => {
              if (entry.path in state) {
                const next = { ...state };
                delete next[entry.path];
                return next;
              }
              void load(entry.path);
              return state;
            });
          }}
        >
          <span className="tree__caret faint" aria-hidden="true">
            {entry.dir ? (entry.path in expanded ? "▾" : "▸") : ""}
          </span>
          <span>{entry.name}</span>
        </button>
        {entry.dir && entry.path in expanded && draw(entry.path, depth + 1)}
      </div>
    ));

  return <div className="tree">{draw("", 0)}</div>;
}

function Search({ where, onOpen }: { where: Where; onOpen: (path: string) => void }) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const go = async (event: React.FormEvent) => {
    event.preventDefault();
    if (query.trim() === "") {
      setHits(null);
      return;
    }
    try {
      setHits(await runSearch(where, query));
      setProblem(null);
    } catch (error: unknown) {
      setHits(null);
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div className="editor__search">
      <form onSubmit={(event) => void go(event)}>
        <input
          aria-label="search"
          placeholder="what does this repo do about…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </form>

      {/* file-sql's own message, not a rewritten one: it says what to run, and a repo
          with no index is a normal state rather than a failure. */}
      {problem !== null && <p className="faint">{problem}</p>}

      {hits !== null && hits.length === 0 && <p className="faint">No hits.</p>}
      {hits?.map((hit, index) => (
        <button
          type="button"
          key={`${hit.path}:${hit.start_line}:${index}`}
          className="editor__hit"
          onClick={() => onOpen(hit.path)}
        >
          <span className="mono">{hit.path}</span>
          <span className="faint">
            {hit.symbol ?? hit.summary ?? `lines ${hit.start_line}-${hit.end_line}`}
          </span>
        </button>
      ))}
    </div>
  );
}
