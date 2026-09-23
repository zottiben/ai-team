import { useCallback, useEffect, useState } from "react";

import { worktrees as fetchWorktrees, type Worktree } from "./api";

/**
 * The worktrees this repository already has.
 *
 * Read from `awt` on every load rather than recorded in ai-team's database: the pool is
 * ai-worktree's (D4), and a copy here would be a second answer that goes wrong the moment
 * somebody runs `awt get` in a terminal.
 *
 * ai-team leases lead with their run/slice/role holder. Human worktrees lead with the
 * branch. The numeric awt pool directory remains path detail rather than identity.
 */
export function Worktrees({ project, tick }: { project: string | null; tick: number }) {
  const [pool, setPool] = useState<Worktree[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (project === null) {
      setPool(null);
      return;
    }
    try {
      setPool(await fetchWorktrees(project));
      setProblem(null);
    } catch (error: unknown) {
      setPool(null);
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  if (project === null) return <p className="empty">Pick a project to see its worktrees.</p>;
  if (problem !== null) return <p className="error">{problem}</p>;
  if (pool === null) return <p className="empty">Reading the pool…</p>;
  if (pool.length === 0) {
    return (
      <p className="empty">
        No worktrees yet. <code className="mono">awt get</code> in the checkout makes one, and
        a run leases one per slice.
      </p>
    );
  }

  return (
    <div className="worktrees">
      {pool.map((tree) => (
        <div key={tree.path} className="worktrees__row">
          <span className="worktrees__identity mono">{semanticName(tree)}</span>
          {activeHolder(tree) !== null && (
            <span className="faint worktrees__branch mono">{tree.branch ?? "detached"}</span>
          )}

          {/* Orphaned is the one state that needs a person: awt will not hand the tree
              out and nobody is holding it, so it sits there costing a slot. */}
          <span className="status" data-status={statusTone(tree)}>
            {isOrphaned(tree) ? "orphaned" : tree.status}
          </span>

          <span className="faint worktrees__where mono">{tree.path}</span>

          {tree.processes.length > 0 && (
            <span className="faint worktrees__procs">
              {tree.processes.length} process{tree.processes.length === 1 ? "" : "es"}:{" "}
              {[...new Set(tree.processes.map((p) => p.name))].join(", ")}
            </span>
          )}

          {isOrphaned(tree) && (
            <span className="faint worktrees__fix mono">awt return {tree.path}</span>
          )}
        </div>
      ))}
    </div>
  );
}

/// Matched on the prefix `awt` writes, not the whole sentence - the rest of it names two
/// commands and is not something to depend on.
function activeHolder(tree: Worktree): string | null {
  return isOrphaned(tree) ? null : (tree.lease_holder ?? null);
}

function semanticName(tree: Worktree): string {
  return activeHolder(tree) ?? tree.branch ?? "detached";
}

function isOrphaned(tree: Worktree): boolean {
  return tree.lease_holder?.startsWith("orphaned:") ?? false;
}

function statusTone(tree: Worktree): string {
  if (isOrphaned(tree)) return "failed";
  if (tree.status === "available") return "done";
  return "running";
}
