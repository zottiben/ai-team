import type { Worktree } from "./api";

/**
 * The checkouts in the order the sidebar draws them, each directly after the one it sits
 * under, with how deep it sits.
 *
 * The server says which checkout sits under which; this only lays that out. A checkout
 * whose parent is not in the list - a stack whose bottom PR has merged and gone - is drawn
 * at the top rather than dropped.
 */
export function nested(trees: Worktree[]): { tree: Worktree; depth: number }[] {
  const known = new Set(trees.map((tree) => tree.path));
  const children = new Map<string | null, Worktree[]>();
  for (const tree of trees) {
    const parent = tree.parent !== null && known.has(tree.parent) ? tree.parent : null;
    children.set(parent, [...(children.get(parent) ?? []), tree]);
  }
  const out: { tree: Worktree; depth: number }[] = [];
  const walk = (parent: string | null, depth: number) => {
    for (const tree of children.get(parent) ?? []) {
      out.push({ tree, depth });
      walk(tree.path, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

/** What a checkout is called in the sidebar: `main`, a PR's key, or its branch. */
export function workspaceName(tree: Worktree): string {
  if (tree.main || tree.kind === "main") return "main";
  if (tree.kind === "pr" && tree.slice_key) return tree.slice_key;
  return tree.branch ?? tree.name;
}

/** The quieter second half of a checkout's name: what it is on, or which plan it is for. */
export function workspaceDetail(tree: Worktree): string | null {
  if (tree.kind === "pr") return tree.plan ?? null;
  if (tree.main || tree.kind === "main") return tree.branch;
  return null;
}

/** A checkout's name as a page heading states it: `main checkout`, `PR2 · <branch>`. */
export function workspaceTitle(tree: Worktree): string {
  if (tree.main || tree.kind === "main") return "main checkout";
  if (tree.kind === "pr" && tree.slice_key) {
    return tree.branch ? `${tree.slice_key} · ${tree.branch}` : tree.slice_key;
  }
  return tree.branch ?? tree.name;
}
