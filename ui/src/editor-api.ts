// The editor's own calls: open with a fingerprint, preview, and a checked save.
//
// Separate from `api.ts` for one reason that matters: a refused save is a 409 carrying
// *both versions of the file*, and the shared request helper turns every non-2xx into an
// Error with a string in it. Flattening a refusal into a message is how "changed on disk"
// becomes "something went wrong" and the operator loses the work they were comparing.

import { token, type Where } from "./api";

export type FileRevision = {
  /** SHA-256 of the exact bytes, hex. */
  hash: string;
  bytes: number;
  device: number;
  inode: number;
  modified_ms: number | null;
};

export type Eol = "none" | "lf" | "crlf" | "mixed";

export type OpenedFile = {
  path: string;
  text: string;
  /** False when the editor refuses it; `reason` always says why. */
  editable: boolean;
  reason: string | null;
  /** Absent exactly when nothing was read, so a buffer cannot claim a base it lacks. */
  revision: FileRevision | null;
  eol: Eol;
  /** What the buffer must be rejoined with to reproduce the file. Null for mixed. */
  line_separator: string | null;
  encoding: string;
  bytes: number;
};

export type ChangeLine = {
  kind: "context" | "added" | "removed";
  before: number | null;
  after: number | null;
  text: string;
};

export type ChangeHunk = {
  before_start: number;
  before_lines: number;
  after_start: number;
  after_lines: number;
  lines: ChangeLine[];
};

export type FileChange = {
  hunks: ChangeHunk[];
  added: number;
  removed: number;
  /** The two sides were too far apart to diff exactly; the hunk is a whole replacement. */
  coarse: boolean;
  truncated: boolean;
};

export type BoundedText = { text: string; truncated: boolean };

export type SavePreview = {
  checkout: string;
  path: string;
  base: FileRevision;
  disk: FileRevision | null;
  /** True when confirming would be refused. The dialog says so before the button. */
  stale: boolean;
  proposed: { hash: string; bytes: number; eol: Eol };
  change: FileChange;
  disk_text: BoundedText | null;
  notices: string[];
};

/**
 * A version the editor displaced, kept where it can be read back.
 *
 * A save swaps two directory entries rather than renaming over one, so the file it
 * replaces is never unlinked - a descriptor another program still holds keeps writing
 * into it. It is kept until the operator discards it, and never by the save itself.
 */
export type PreservedOriginal = {
  /** The store-relative name `discardKept` takes. */
  name: string;
  /** Exactly where it is. */
  path: string;
  bytes: number;
  hash: string;
  /** True when these are not the bytes the editor had open - another writer's work. */
  foreign: boolean;
  /** True when it had to stay in the checkout, where `git status` will show it. */
  in_checkout: boolean;
};

export type Saved = {
  path: string;
  revision: FileRevision;
  replaced: FileRevision;
  /** Where the version this replaced is kept, or null when it could not be kept. */
  preserved: PreservedOriginal | null;
  preserve_note: string | null;
  /** How many chats will be told about this edit when they next take a turn. */
  notified_chats: number;
};

export type RefusalReason =
  | "changed_on_disk"
  | "replaced_on_disk"
  | "missing"
  | "not_a_plain_file"
  | "not_the_preview"
  | "too_large"
  | "lost_a_race"
  /** The filesystem cannot swap two entries in one step, so a save there is refused. */
  | "no_atomic_exchange"
  /** The swap happened and could not be undone. Nothing was deleted; `preserved` says
   * what is on disk. */
  | "needs_recovery";

export type Refusal = {
  reason: RefusalReason;
  message: string;
  disk: FileRevision | null;
  disk_text: BoundedText | null;
  /** Files the refusal left on disk. Empty for a refusal that changed nothing. */
  preserved: PreservedOriginal[];
};

/** One kept version, as the recovery store lists it. */
export type KeptFile = {
  name: string;
  path: string;
  /** The checkout-relative path it was displaced from. */
  origin: string;
  bytes: number;
  kept_at: string | null;
};

export type SaveResult = { saved: Saved } | { refused: Refusal };

export type Baseline = { path: string; text: string | null; tracked: boolean };

function scope(where: Where, extra: Record<string, string>): string {
  const params = new URLSearchParams({ project: where.project, ...extra });
  if (where.workspace !== null && where.workspace !== undefined) {
    params.set("workspace", where.workspace);
  }
  if (where.node !== null && where.node !== undefined) params.set("node", String(where.node));
  return params.toString();
}

async function read<T>(path: string): Promise<T> {
  const current = token();
  const response = await fetch(`/api${path}`, {
    headers: current ? { "x-ai-team-token": current } : {},
  });
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new Error(problem?.error ?? `${path} failed with ${response.status}`);
  }
  return (await response.json()) as T;
}

async function write(path: string, body: unknown): Promise<Response> {
  const current = token();
  return fetch(`/api${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(current ? { "x-ai-team-token": current } : {}),
    },
    body: JSON.stringify(body),
  });
}

export function openFile(where: Where, path: string): Promise<OpenedFile> {
  return read<OpenedFile>(`/editor/open?${scope(where, { path })}`);
}

/** The committed text the change markers compare against. */
export function baseline(where: Where, path: string): Promise<Baseline> {
  return read<Baseline>(`/editor/baseline?${scope(where, { path })}`);
}

export async function previewSave(
  where: Where,
  path: string,
  base: FileRevision,
  text: string,
): Promise<SavePreview> {
  const response = await write("/editor/preview", { ...where, path, base, text });
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new Error(problem?.error ?? `preview failed with ${response.status}`);
  }
  return (await response.json()) as SavePreview;
}

/**
 * Confirm a previewed save.
 *
 * `proposedHash` is the preview's own hash. The server checks the body against it, so a
 * confirmation can never be applied to bytes the operator did not see.
 */
export async function saveFile(
  where: Where,
  path: string,
  base: FileRevision,
  proposedHash: string,
  text: string,
): Promise<SaveResult> {
  const response = await write("/editor/save", {
    ...where,
    path,
    base,
    proposed_hash: proposedHash,
    text,
  });
  if (response.status === 409) {
    return { refused: (await response.json()) as Refusal };
  }
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new Error(problem?.error ?? `save failed with ${response.status}`);
  }
  return { saved: (await response.json()) as Saved };
}

/** Every version this checkout's editor has displaced, newest first. */
export function keptFiles(where: Where): Promise<KeptFile[]> {
  return read<KeptFile[]>(`/editor/kept?${scope(where, {})}`);
}

/** The text of one kept version. Null when it is not there any more. */
export async function keptText(where: Where, name: string): Promise<BoundedText | null> {
  const body = await read<{ name: string; text: BoundedText | null }>(
    `/editor/kept/text?${scope(where, { name })}`,
  );
  return body.text;
}

/**
 * Throw one kept version away.
 *
 * Only ever from here. The save path keeps what it displaced precisely because it cannot
 * know whether another program is still writing into it, so discarding is the operator's
 * decision and nobody else's.
 */
export async function discardKept(where: Where, name: string): Promise<boolean> {
  const response = await write("/editor/kept/discard", { ...where, name });
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new Error(problem?.error ?? `discard failed with ${response.status}`);
  }
  return ((await response.json()) as { discarded: boolean }).discarded;
}
