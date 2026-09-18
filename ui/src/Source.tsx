import { useCallback, useEffect, useState } from "react";

import {
  scm as fetchScm,
  scmBranch,
  scmCommit,
  scmPush,
  scmStage,
  type FileDiff,
  type Scm,
  type Where,
} from "./api";

/**
 * Source control over the active worktree.
 *
 * Unstaged and staged are shown apart rather than summed. Their sum cannot tell you what
 * pressing commit would actually record, which is the one question this view exists to
 * answer.
 *
 * The diff model is the same one Review renders - parsed from git's own output by the
 * server - so there is one implementation of what a hunk is and one place a line number
 * can be wrong.
 */
export function Source({ project, node }: { project: string | null; node: number | null }) {
  const [data, setData] = useState<Scm | null>(null);
  const [message, setMessage] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const where: Where | null = project === null ? null : { project, node };

  const load = useCallback(async () => {
    if (where === null) return;
    try {
      setData(await fetchScm(where));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, node]);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (what: () => Promise<unknown>, said?: string) => {
    try {
      await what();
      setProblem(null);
      if (said !== undefined) setNote(said);
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  if (project === null || where === null) {
    return <p className="empty">Pick a project to see its changes.</p>;
  }
  if (data === null) return <p className="empty">Reading the worktree…</p>;

  const nothing =
    data.unstaged.length === 0 && data.staged.length === 0 && data.untracked.length === 0;

  return (
    <div className="source">
      <div className="main__header">
        <h2>Source control</h2>
        <label className="source__branch">
          <span className="faint">on</span>
          <select
            aria-label="branch"
            value={data.branch ?? ""}
            onChange={(event) => void act(() => scmBranch(where, event.target.value))}
          >
            {data.branch === null && <option value="">detached</option>}
            {data.branches.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="button" onClick={() => void act(() => scmPush(where), "Pushed.")}>
          Push
        </button>
      </div>

      {problem !== null && <p className="error">{problem}</p>}
      {note !== null && <p className="notice">{note}</p>}

      {nothing && <p className="empty">Nothing has changed in this worktree.</p>}

      {data.staged.length > 0 && (
        <section>
          <div className="dock__title">Staged - this is what a commit would record</div>
          {data.staged.map((file) => (
            <Changed
              key={file.path}
              file={file}
              staged
              onFile={() => void act(() => scmStage(where, { path: file.path, unstage: true }))}
            />
          ))}
        </section>
      )}

      {data.unstaged.length > 0 && (
        <section>
          <div className="dock__title">Changed</div>
          {data.unstaged.map((file) => (
            <Changed
              key={file.path}
              file={file}
              onFile={() => void act(() => scmStage(where, { path: file.path }))}
              onHunk={(index) => void act(() => scmStage(where, { path: file.path, hunk: index }))}
            />
          ))}
        </section>
      )}

      {data.untracked.length > 0 && (
        <section>
          {/* Untracked files have no diff at all, so a view built only from `git diff`
              leaves an agent's new file out of the commit entirely. */}
          <div className="dock__title">Untracked</div>
          {data.untracked.map((path) => (
            <div key={path} className="card__row">
              <span className="mono">{path}</span>
              <button
                type="button"
                className="button"
                onClick={() => void act(() => scmStage(where, { path }))}
              >
                Stage
              </button>
            </div>
          ))}
        </section>
      )}

      <form
        className="source__commit"
        onSubmit={(event) => {
          event.preventDefault();
          if (message.trim() === "") return;
          void act(async () => {
            const { sha } = await scmCommit(where, message);
            setMessage("");
            setNote(`Committed ${sha.slice(0, 7)}.`);
          });
        }}
      >
        <textarea
          aria-label="commit message"
          placeholder={"feat: what changed\n\nWhy, if it is not obvious."}
          rows={3}
          value={message}
          onChange={(event) => setMessage(event.target.value)}
        />
        <button
          type="submit"
          className="button button--primary"
          disabled={data.staged.length === 0 || message.trim() === ""}
        >
          Commit
        </button>
      </form>
    </div>
  );
}

function Changed({
  file,
  staged = false,
  onFile,
  onHunk,
}: {
  file: FileDiff;
  staged?: boolean;
  onFile: () => void;
  onHunk?: (index: number) => void;
}) {
  const [open, setOpen] = useState(false);

  return (
    <div className="source__file">
      <div className="card__row">
        <button type="button" className="source__name" onClick={() => setOpen(!open)}>
          <span className="faint">{open ? "▾" : "▸"}</span>
          <span className="mono">{file.path}</span>
          <span className="faint mono">
            +{file.additions} -{file.deletions}
          </span>
        </button>
        <button type="button" className="button" onClick={onFile}>
          {staged ? "Unstage" : "Stage file"}
        </button>
      </div>

      {open &&
        file.hunks.map((hunk, index) => (
          <div key={hunk.header} className="source__hunk">
            <div className="card__row">
              <span className="diff__hunk-head mono">{hunk.header}</span>
              {onHunk !== undefined && (
                <button type="button" className="button" onClick={() => onHunk(index)}>
                  Stage hunk
                </button>
              )}
            </div>
            {hunk.lines.map((line, at) => (
              <div key={`${hunk.header}:${at}`} className="diff__line" data-kind={line.kind}>
                <span className="diff__num mono">{line.old ?? ""}</span>
                <span className="diff__num mono">{line.new ?? ""}</span>
                <span />
                <code className="diff__text">{line.text === "" ? " " : line.text}</code>
              </div>
            ))}
          </div>
        ))}
    </div>
  );
}
