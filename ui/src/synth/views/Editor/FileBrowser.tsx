import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  repoMap,
  search as runSearch,
  tree as fetchTree,
  type Hit,
  type Scm,
  type TreeEntry,
  type Where,
} from "../../../api";
import { Lcd } from "../../primitives/Lcd";
import { Led } from "../../primitives/Led";
import { presentAt, type Presence } from "./presence";

/** §11.2: M Yellow, U Ice, A Cyan, D Sunset. */
export type GitLetter = "M" | "U" | "A" | "D";

/**
 * One letter per path, from what git actually reports.
 *
 * Staged and unstaged are two different diffs, and a file can be in both. The letter
 * shows the stronger statement - a delete over a modification - rather than whichever
 * list happened to be read last.
 */
export function letters(scm: Scm | null): Map<string, GitLetter> {
  const out = new Map<string, GitLetter>();
  if (scm === null) return out;
  for (const path of scm.untracked) out.set(path, "U");
  for (const diff of [...scm.staged, ...scm.unstaged]) {
    const letter: GitLetter = diff.status === "removed" ? "D" : diff.status === "added" ? "A" : "M";
    const seen = out.get(diff.path);
    if (seen !== "D") out.set(diff.path, letter);
  }
  return out;
}

type Row = { path: string; name: string; dir: boolean; depth: number; expanded: boolean };

/** The visible rows, flattened, which is what arrow keys move through. */
function flatten(
  levels: Record<string, TreeEntry[]>,
  path = "",
  depth = 0,
): Row[] {
  return (levels[path] ?? []).flatMap(entry => {
    const expanded = entry.dir && entry.path in levels;
    return [
      { path: entry.path, name: entry.name, dir: entry.dir, depth, expanded },
      ...(expanded ? flatten(levels, entry.path, depth + 1) : []),
    ];
  });
}

export default function FileBrowser({ where, checkout, scm, presence, active, visible, onOpen }: {
  where: Where;
  checkout: string;
  scm: Scm | null;
  presence: Presence;
  active: string | null;
  /** The editor pane is on screen. ⌘P belongs to the browser's address bar otherwise. */
  visible: boolean;
  onOpen: (path: string) => void;
}) {
  const [levels, setLevels] = useState<Record<string, TreeEntry[]>>({});
  const [problem, setProblem] = useState<string | null>(null);
  const [mode, setMode] = useState<"files" | "content">("files");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[] | null>(null);
  const [searchProblem, setSearchProblem] = useState<string | null>(null);
  const [index, setIndex] = useState<{ paths: string[]; truncated: boolean } | null>(null);
  const [cursor, setCursor] = useState(0);
  const list = useRef<HTMLDivElement>(null);
  const find = useRef<HTMLInputElement>(null);

  const load = useCallback(
    async (path: string) => {
      try {
        const entries = await fetchTree(where, path);
        setLevels(state => ({ ...state, [path]: entries }));
        setProblem(null);
      } catch (error: unknown) {
        setProblem(error instanceof Error ? error.message : String(error));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [where.project, where.workspace, where.node],
  );

  useEffect(() => {
    void load("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [where.project, where.workspace, where.node]);

  // The filename index is the repository map, which the server already walks for the
  // Map view. Bounded there, and it says when it was cut rather than quietly ending.
  useEffect(() => {
    let live = true;
    void repoMap(where.project, where.workspace)
      .then(map => {
        if (live) {
          setIndex({
            paths: map.nodes.filter(node => !node.dir && node.path !== "").map(node => node.path),
            truncated: map.truncated,
          });
        }
      })
      .catch(() => setIndex(null));
    return () => { live = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [where.project, where.workspace]);

  // ⌘P, the shortcut the screen advertises. Taken only while the editor is the visible
  // tool, so it is the browser's own Print everywhere else in the app.
  useEffect(() => {
    if (!visible) return undefined;
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "p") {
        event.preventDefault();
        setMode("files");
        find.current?.focus();
        find.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible]);

  const status = useMemo(() => letters(scm), [scm]);
  const rows = useMemo(() => flatten(levels), [levels]);

  const matches = useMemo(() => {
    if (mode !== "files" || query.trim() === "" || index === null) return [];
    const needle = query.trim().toLowerCase();
    return index.paths.filter(path => path.toLowerCase().includes(needle)).slice(0, 40);
  }, [mode, query, index]);

  const toggle = (row: Row) => {
    if (!row.dir) { onOpen(row.path); return; }
    setLevels(state => {
      if (row.path in state) {
        const next = { ...state };
        delete next[row.path];
        return next;
      }
      void load(row.path);
      return state;
    });
  };

  const onKey = (event: React.KeyboardEvent) => {
    const row = rows[cursor];
    if (event.key === "ArrowDown") { event.preventDefault(); setCursor(at => Math.min(rows.length - 1, at + 1)); }
    else if (event.key === "ArrowUp") { event.preventDefault(); setCursor(at => Math.max(0, at - 1)); }
    else if (event.key === "ArrowRight" && row?.dir && !row.expanded) { event.preventDefault(); toggle(row); }
    else if (event.key === "ArrowLeft" && row?.dir && row.expanded) { event.preventDefault(); toggle(row); }
    else if (event.key === "Enter" && row !== undefined) { event.preventDefault(); toggle(row); }
    else if (event.key === "Home") { event.preventDefault(); setCursor(0); }
    else if (event.key === "End") { event.preventDefault(); setCursor(Math.max(0, rows.length - 1)); }
  };

  const runContentSearch = async (event: React.FormEvent) => {
    event.preventDefault();
    if (mode === "files") {
      const first = matches[0];
      if (first !== undefined) onOpen(first);
      return;
    }
    if (query.trim() === "") { setHits(null); return; }
    try {
      setHits(await runSearch(where, query));
      setSearchProblem(null);
    } catch (error: unknown) {
      setHits(null);
      setSearchProblem(error instanceof Error ? error.message : String(error));
    }
  };

  return <div className="synth-editor__browser">
    <form className="synth-editor__find" onSubmit={event => void runContentSearch(event)}>
      <Lcd grid className="synth-editor__find-screen">
        <input
          ref={find}
          aria-label={mode === "files" ? "Go to file" : "Search file contents"}
          placeholder={mode === "files" ? "Go to file… ⌘P" : "what does this repo do about…"}
          value={query}
          onChange={event => setQuery(event.target.value)}
        />
      </Lcd>
      <div className="synth-editor__find-modes" role="group" aria-label="Search mode">
        {(["files", "content"] as const).map(name => <button
          type="button"
          key={name}
          className="synth-editor__find-mode"
          aria-pressed={mode === name}
          onClick={() => { setMode(name); setHits(null); }}
        >{name === "files" ? "Files" : "Content"}</button>)}
      </div>
    </form>

    <div className="synth-editor__where synth-mono">
      <span title={checkout}>{checkout}</span>
      <span className="synth-editor__where-branch">{scm?.branch ?? "detached"}</span>
    </div>

    {mode === "files" && query.trim() !== "" && <div className="synth-editor__matches">
      {index === null && <p className="synth-muted">No file index for this checkout.</p>}
      {matches.map(path => <button type="button" key={path} className="synth-editor__match synth-mono" onClick={() => onOpen(path)}>
        {path}
      </button>)}
      {index?.truncated === true && <p className="synth-muted">The walk was cut; some files are not listed.</p>}
    </div>}

    {mode === "content" && <div className="synth-editor__matches">
      {/* file-sql's own message, not a rewritten one: a repo with no index is a normal
          state rather than a failure, and it says what to run. */}
      {searchProblem !== null && <p className="synth-muted">{searchProblem}</p>}
      {hits !== null && hits.length === 0 && <p className="synth-muted">No hits.</p>}
      {hits?.map((hit, at) => <button
        type="button"
        key={`${hit.path}:${hit.start_line}:${at}`}
        className="synth-editor__match synth-mono"
        onClick={() => onOpen(hit.path)}
      >
        <span>{hit.path}</span>
        <span className="synth-muted">{hit.symbol ?? hit.summary ?? `lines ${hit.start_line}-${hit.end_line}`}</span>
      </button>)}
    </div>}

    {problem !== null && <p className="synth-muted">{problem}</p>}

    <div
      ref={list}
      className="synth-editor__tree"
      role="tree"
      aria-label="Files"
      tabIndex={0}
      onKeyDown={onKey}
    >
      {rows.map((row, at) => <div
        key={row.path}
        role="treeitem"
        aria-selected={row.path === active}
        aria-expanded={row.dir ? row.expanded : undefined}
        aria-level={row.depth + 1}
        className="synth-editor__row"
        data-active={row.path === active || undefined}
        data-cursor={at === cursor || undefined}
        style={{ paddingLeft: `${row.depth * 12 + 8}px` }}
        onClick={() => { setCursor(at); toggle(row); }}
      >
        <span className="synth-editor__caret" aria-hidden="true">{row.dir ? (row.expanded ? "▾" : "▸") : ""}</span>
        {presentAt(presence, row.path, row.dir)
          ? <Led state="pulse" signal="diff" size={6} />
          : <span className="synth-editor__led-slot" aria-hidden="true" />}
        <span className="synth-editor__name synth-mono">{row.name}</span>
        {status.has(row.path) && <span className="synth-editor__git" data-letter={status.get(row.path)}>
          {status.get(row.path)}
        </span>}
      </div>)}
    </div>

    <div className="synth-editor__legend synth-mono" aria-label="Legend">
      <span data-letter="M">M modified</span>
      <span data-letter="U">U untracked</span>
      <span data-letter="A">A added</span>
      <span data-letter="D">D deleted</span>
      <span><Led state="pulse" signal="diff" size={6} /> agent, last 5 min</span>
    </div>
  </div>;
}
