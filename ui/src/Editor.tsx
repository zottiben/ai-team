import { EditorView } from "@codemirror/view";
import type { EditorState, Extension } from "@codemirror/state";
import { Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";

import {
  search as runSearch,
  tree as fetchTree,
  type Hit,
  type TreeEntry,
  type Where,
} from "./api";
import {
  discardKept,
  openFile,
  previewSave,
  saveFile,
  type FileRevision,
  type OpenedFile,
  type PreservedOriginal,
  type Refusal,
  type SavePreview as Preview,
} from "./editor-api";
import { changesSlot, grammarSlot, loadGrammar, stateFor, themeSlot } from "./codemirror";
import { languageServer } from "./language";
import { useSynth } from "./ui-preferences";

const SynthWorkspace = lazy(() => import("./synth/views/Editor/EditorWorkspace"));

/**
 * One open file.
 *
 * `saved` is the text of the revision in `base` - what is on disk as far as this window
 * knows - so dirty is a comparison rather than a guess, and `base` is what the server
 * checks a save against. A buffer with no `base` was never read and can never be saved.
 */
export type Buffer = {
  path: string;
  text: string;
  saved: string;
  base: FileRevision | null;
  editable: boolean;
  reason: string | null;
  eol: OpenedFile["eol"];
  separator: string | null;
  encoding: string;
  /** Bumped to rebuild the CodeMirror state after a revert or a reload from disk. */
  generation: number;
};

/** What a save is waiting on: the preview the operator is being shown. */
export type Pending = { path: string; text: string; preview: Preview };

/**
 * What a save actually did, including what it did with the version it replaced.
 *
 * `kept` is not an afterthought: the backend swaps the two files rather than renaming
 * over one, so the previous version is still on disk. Saying so - with somewhere to
 * discard it from - is the difference between keeping a file and hiding one.
 */
export type Receipt = {
  path: string;
  bytes: number;
  chats: number;
  kept: PreservedOriginal | null;
  note: string | null;
};

export function Editor({
  project,
  workspace = null,
  node,
  chat = null,
  visible = true,
  onOpenChat,
  onSteer,
}: {
  project: string | null;
  workspace?: string | null;
  node: number | null;
  /** The chat whose checkout this is, when there is one. What agent presence reads. */
  chat?: number | null;
  /** Keep buffers mounted while hidden, without intercepting another pane's Save. */
  visible?: boolean;
  onOpenChat?: () => void;
  onSteer?: () => void;
}) {
  const synth = useSynth();
  const [open, setOpen] = useState<Buffer[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [refusal, setRefusal] = useState<{ path: string; refusal: Refusal } | null>(null);
  const [reverting, setReverting] = useState<string | null>(null);
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const [busy, setBusy] = useState(false);

  // CodeMirror owns the document, and its undo history lives in the state. Keeping the
  // state here is what lets a tab switch - or a skin toggle that re-parents the DOM -
  // come back with the history intact instead of a fresh buffer.
  const states = useRef(new Map<string, EditorState>());

  const where: Where | null = project === null ? null : { project, workspace, node };
  const current = open.find((buffer) => buffer.path === active) ?? null;

  const openPath = useCallback(
    async (path: string) => {
      if (where === null) return;
      setActive(path);
      // Already open: switching tabs must not discard unsaved edits by re-reading.
      if (open.some((buffer) => buffer.path === path)) return;
      try {
        const body = await openFile(where, path);
        setOpen((buffers) =>
          buffers.some((buffer) => buffer.path === path)
            ? buffers
            : [
                ...buffers,
                {
                  path,
                  text: body.text,
                  saved: body.text,
                  base: body.revision,
                  editable: body.editable,
                  reason: body.reason,
                  eol: body.eol,
                  separator: body.line_separator,
                  encoding: body.encoding,
                  generation: 0,
                },
              ],
        );
        setProblem(null);
      } catch (error: unknown) {
        setProblem(error instanceof Error ? error.message : String(error));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [project, workspace, node, open],
  );

  const closePath = useCallback(
    (path: string) => {
      forget(states.current, path);
      setOpen((buffers) => {
        const index = buffers.findIndex((entry) => entry.path === path);
        // A neighbour takes the closed file's place - the next tab, else the one before -
        // rather than an empty pane beside tabs still open.
        if (active === path) setActive((buffers[index + 1] ?? buffers[index - 1])?.path ?? null);
        return buffers.filter((entry) => entry.path !== path);
      });
    },
    [active],
  );

  const edit = useCallback((path: string, change: (buffer: Buffer) => Buffer) => {
    setOpen((buffers) => buffers.map((buffer) => (buffer.path === path ? change(buffer) : buffer)));
  }, []);

  /** Step one of a save: ask the server exactly what confirming would write. */
  const requestSave = useCallback(async () => {
    if (where === null || current === null || current.base === null) return;
    if (current.text === current.saved) return;
    setBusy(true);
    try {
      const preview = await previewSave(where, current.path, current.base, current.text);
      setPending({ path: current.path, text: current.text, preview });
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, workspace, node, current]);

  /** Step two: write, or carry the refusal back with both versions intact. */
  const confirmSave = useCallback(async () => {
    if (where === null || pending === null) return;
    const buffer = open.find((entry) => entry.path === pending.path);
    if (buffer?.base === undefined || buffer.base === null) return;
    setBusy(true);
    try {
      const result = await saveFile(
        where,
        pending.path,
        buffer.base,
        pending.preview.proposed.hash,
        pending.text,
      );
      if ("refused" in result) {
        setRefusal({ path: pending.path, refusal: result.refused });
        setPending(null);
        return;
      }
      edit(pending.path, (entry) => ({
        ...entry,
        saved: pending.text,
        base: result.saved.revision,
      }));
      setPending(null);
      setRefusal(null);
      setReceipt({
        path: pending.path,
        bytes: result.saved.revision.bytes,
        chats: result.saved.notified_chats,
        kept: result.saved.preserved ?? null,
        note: result.saved.preserve_note ?? null,
      });
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
      setPending(null);
    } finally {
      setBusy(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, workspace, node, pending, open, edit]);

  /** Take the other writer's version, replacing the buffer and the base together. */
  const reloadFromDisk = useCallback(
    async (path: string) => {
      if (where === null) return;
      try {
        const body = await openFile(where, path);
        edit(path, (entry) => ({
          ...entry,
          text: body.text,
          saved: body.text,
          base: body.revision,
          editable: body.editable,
          reason: body.reason,
          eol: body.eol,
          separator: body.line_separator,
          generation: entry.generation + 1,
        }));
        setRefusal(null);
        setProblem(null);
      } catch (error: unknown) {
        setProblem(error instanceof Error ? error.message : String(error));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [project, workspace, node, edit],
  );

  /**
   * Throw away one kept version, on the operator's say-so.
   *
   * Nothing on the save path does this. A kept version is the only copy of whatever
   * another program wrote into that file after the swap, and only a person can say that
   * nobody wants it.
   */
  const discardVersion = useCallback(
    async (name: string) => {
      if (where === null) return;
      try {
        await discardKept(where, name);
        setReceipt((current) => (current === null ? null : { ...current, kept: null }));
        setProblem(null);
      } catch (error: unknown) {
        setProblem(error instanceof Error ? error.message : String(error));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [project, workspace, node],
  );

  /** Revert touches the buffer only. Nothing on disk changes, and it is confirmed. */
  const revert = useCallback(
    (path: string) => {
      edit(path, (entry) => ({
        ...entry,
        text: entry.saved,
        generation: entry.generation + 1,
      }));
      setReverting(null);
    },
    [edit],
  );

  // Ctrl/Cmd-S, because that is the muscle memory and the browser's own Save is useless
  // here. It opens the preview; it has never been the thing that writes.
  useEffect(() => {
    if (!visible) return undefined;
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "s") {
        event.preventDefault();
        void requestSave();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [requestSave, visible]);

  if (project === null) {
    return <p className="empty">Pick a project to open its files.</p>;
  }

  const scope: Where = { project, workspace, node };
  const surface =
    current === null ? null : (
      <Surface
        key={`${current.path}:${current.generation}`}
        where={scope}
        path={current.path}
        generation={current.generation}
        text={current.saved}
        separator={current.separator}
        theme={synth}
        cache={states.current}
        onChange={(text) => edit(current.path, (buffer) => ({ ...buffer, text }))}
      />
    );

  if (synth) {
    return (
      <Suspense fallback={<p className="empty">Loading editor…</p>}>
        <SynthWorkspace
          where={scope}
          chat={chat}
          visible={visible}
          buffers={open}
          active={active}
          current={current}
          problem={problem}
          pending={pending}
          refusal={refusal}
          reverting={reverting}
          receipt={receipt}
          busy={busy}
          surface={surface}
          onOpen={(path) => void openPath(path)}
          onSelect={setActive}
          onClose={closePath}
          onSave={() => void requestSave()}
          onConfirm={() => void confirmSave()}
          onCancel={() => setPending(null)}
          onReload={(path) => void reloadFromDisk(path)}
          onDismissRefusal={() => setRefusal(null)}
          onAskRevert={setReverting}
          onRevert={revert}
          onDismissReceipt={() => setReceipt(null)}
          onDiscardKept={(name) => void discardVersion(name)}
          onOpenChat={onOpenChat}
          onSteer={onSteer}
        />
      </Suspense>
    );
  }

  return (
    <div className="editor">
      <div className="editor__side">
        <Search where={scope} onOpen={(path) => void openPath(path)} />
        <Tree where={scope} onOpen={(path) => void openPath(path)} />
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
                    closePath(buffer.path);
                  }}
                >
                  ×
                </span>
              </button>
            ))}
            {current !== null && (
              <>
                <button
                  type="button"
                  className="button editor__revert"
                  disabled={current.text === current.saved}
                  onClick={() => setReverting(current.path)}
                >
                  Revert
                </button>
                <button
                  type="button"
                  className="button button--primary editor__save"
                  disabled={current.text === current.saved || busy || current.base === null}
                  onClick={() => void requestSave()}
                >
                  Save
                </button>
              </>
            )}
          </div>
        )}

        {problem !== null && <p className="error">{problem}</p>}
        {receipt !== null && (
          <p className="notice" role="status">
            Saved {receipt.path} ({receipt.bytes} bytes).{" "}
            {receipt.chats > 0
              ? `${receipt.chats} chat${receipt.chats === 1 ? "" : "s"} will be told at the next turn.`
              : "No chat works in this checkout, so nothing was told."}{" "}
            {receipt.kept !== null && (
              <>
                The version it replaced is kept at{" "}
                <span className="mono">{receipt.kept.path}</span>
                {receipt.kept.in_checkout ? " - inside the checkout, so git will see it." : "."}{" "}
                <button
                  type="button"
                  className="button"
                  onClick={() => void discardVersion(receipt.kept?.name ?? "")}
                >
                  Discard kept version
                </button>{" "}
              </>
            )}
            {receipt.note !== null && <span className="faint">{receipt.note} </span>}
            <button type="button" className="button" onClick={() => setReceipt(null)}>
              Dismiss
            </button>
          </p>
        )}

        {reverting !== null && (
          <div className="notice" role="dialog" aria-label="Confirm revert">
            <p>
              Throw away unsaved changes to {reverting}? Nothing on disk changes - only this
              buffer goes back to the version that was read.
            </p>
            <button type="button" className="button button--primary" onClick={() => revert(reverting)}>
              Discard my changes
            </button>
            <button type="button" className="button" onClick={() => setReverting(null)}>
              Keep editing
            </button>
          </div>
        )}

        {refusal !== null && (
          <LegacyRefusal
            refusal={refusal}
            onReload={() => void reloadFromDisk(refusal.path)}
            onDismiss={() => setRefusal(null)}
          />
        )}

        {pending !== null && (
          <LegacyPreview
            pending={pending}
            busy={busy}
            onConfirm={() => void confirmSave()}
            onCancel={() => setPending(null)}
          />
        )}

        {current === null ? (
          <p className="empty">Open a file from the tree, or search for one.</p>
        ) : current.editable ? (
          surface
        ) : (
          // Refused rather than rendered: a lossy read followed by a save would replace
          // every undecodable byte and quietly corrupt the file.
          <p className="empty">
            {current.path} is {current.reason ?? "not text"} - not opening it for editing.
          </p>
        )}
      </div>
    </div>
  );
}

function LegacyPreview({
  pending,
  busy,
  onConfirm,
  onCancel,
}: {
  pending: Pending;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { preview } = pending;
  return (
    <div className="notice" role="dialog" aria-label="Confirm save">
      <p>
        <strong>Save {preview.path}</strong> into <span className="mono">{preview.checkout}</span>:{" "}
        {preview.proposed.bytes} bytes, +{preview.change.added} −{preview.change.removed}.
      </p>
      {preview.stale ? (
        <p className="error">Changed on disk since you opened it - reload or compare</p>
      ) : null}
      {preview.notices.map((notice) => (
        <p className="faint" key={notice}>
          {notice}
        </p>
      ))}
      <pre className="mono editor__preview">
        {preview.change.hunks
          .flatMap((hunk) =>
            hunk.lines.map(
              (line) =>
                `${line.kind === "added" ? "+" : line.kind === "removed" ? "-" : " "}${line.text}`,
            ),
          )
          .join("\n")}
      </pre>
      {preview.change.truncated && <p className="faint">Preview cut - more hunks than shown.</p>}
      <button
        type="button"
        className="button button--primary"
        disabled={busy || preview.stale}
        onClick={onConfirm}
      >
        Write these bytes
      </button>
      <button type="button" className="button" onClick={onCancel}>
        Cancel
      </button>
    </div>
  );
}

function LegacyRefusal({
  refusal,
  onReload,
  onDismiss,
}: {
  refusal: { path: string; refusal: Refusal };
  onReload: () => void;
  onDismiss: () => void;
}) {
  return (
    <div className="error" role="alert">
      <p>{refusal.refusal.message}</p>
      {refusal.refusal.disk_text !== null && (
        <details>
          <summary>What is on disk now</summary>
          <pre className="mono editor__preview">{refusal.refusal.disk_text.text}</pre>
          {refusal.refusal.disk_text.truncated && <p className="faint">Shown in part.</p>}
        </details>
      )}
      {(refusal.refusal.preserved ?? []).map((kept) => (
        <p className="faint" key={kept.name}>
          Kept, not deleted: <span className="mono">{kept.path}</span> ({kept.bytes} bytes).
        </p>
      ))}
      <button type="button" className="button" onClick={onReload}>
        Reload from disk
      </button>
      <button type="button" className="button" onClick={onDismiss}>
        Keep my version
      </button>
    </div>
  );
}

/** Drop every cached state for a path, whatever generation it was saved under. */
function forget(cache: Map<string, EditorState>, path: string): void {
  for (const key of [...cache.keys()]) {
    if (key === path || key.startsWith(`${path}:`)) cache.delete(key);
  }
}

/**
 * The CodeMirror instance.
 *
 * Mounted once per path and generation, because CodeMirror owns its document - swapping
 * the text underneath a live view would discard the undo history along with it. The state
 * is handed back to the cache on unmount, so the same buffer reopens where it was left,
 * including its history, after a tab switch or a change of skin.
 */
function Surface({
  where,
  path,
  generation,
  text,
  separator,
  theme,
  cache,
  onChange,
}: {
  where: Where;
  path: string;
  generation: number;
  text: string;
  separator: string | null;
  theme: boolean;
  cache: Map<string, EditorState>;
  onChange: (text: string) => void;
}) {
  const host = useRef<HTMLDivElement | null>(null);
  const latest = useRef(onChange);
  latest.current = onChange;

  useEffect(() => {
    if (host.current === null) return undefined;
    // Keyed by generation as well as path. A revert bumps the generation, and that is
    // what makes the discarded state unreachable: the outgoing view writes its state
    // back on unmount, *after* the replacement has been asked for, so deleting the
    // entry up front would hand the new buffer the text it was told to throw away.
    const slot = `${path}:${generation}`;
    const kept = cache.get(slot);
    const view = new EditorView({
      state:
        kept ??
        stateFor(
          text,
          (value) => latest.current(value),
          languageServer(where, path),
          [],
          separator,
        ),
      parent: host.current,
    });
    let live = true;
    // The grammar is a separate chunk: the buffer is editable immediately and gains its
    // colours when the download lands, rather than waiting on PHP to open a README.
    void loadGrammar(path).then((grammar) => {
      if (live && grammar !== null) {
        view.dispatch({ effects: grammarSlot.reconfigure(grammar) });
      }
    });
    return () => {
      live = false;
      cache.set(slot, view.state);
      view.destroy();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, generation]);

  // The skin changes the theme, not the document: reconfiguring keeps the buffer, the
  // selection and the undo history exactly where they were.
  useEffect(() => {
    let cancelled = false;
    const apply = (extension: Extension) => {
      const instance = host.current === null ? null : EditorView.findFromDOM(host.current);
      if (!cancelled && instance !== null) {
        instance.dispatch({ effects: themeSlot.reconfigure(extension) });
      }
    };
    if (theme) {
      void import("./synth/editor-theme").then((module) => apply(module.synthEditorTheme()));
    } else {
      // The change-marker gutter belongs to the synth skin, and it rides in the state -
      // which is cached and restored - so switching back has to take it out again or
      // the legacy editor keeps an empty column it has no styles for.
      apply([]);
      const instance = host.current === null ? null : EditorView.findFromDOM(host.current);
      instance?.dispatch({ effects: changesSlot.reconfigure([]) });
    }
    return () => {
      cancelled = true;
    };
  }, [theme, path]);

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

export { Search, Tree };
