import { RangeSetBuilder, StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, EditorView, GutterMarker, gutter } from "@codemirror/view";

/**
 * What this buffer changes about the committed file (SPEC §11.2).
 *
 * Computed in the window rather than read from `git diff`, because the markers have to
 * follow the buffer as it is typed - a diff of what is on disk would describe a file
 * nobody is looking at. The committed text comes from the server once per open.
 *
 * Line numbers are one-based, matching the gutter.
 */
export type Changes = {
  /** Lines in this buffer that HEAD does not have. */
  added: Set<number>;
  /** Lines this buffer *follows* a removal with. The wedge sits under the line above. */
  removedAfter: Set<number>;
  /** How many lines were removed, which the marker positions cannot say on their own. */
  removedCount: number;
  /** True when the two sides were too far apart to compare exactly. No guessing. */
  coarse: boolean;
  /** Null when the file is not in HEAD at all; every line is new, and that is not a diff. */
  untracked: boolean;
};

export const NO_CHANGES: Changes = {
  added: new Set(),
  removedAfter: new Set(),
  removedCount: 0,
  coarse: false,
  untracked: false,
};

/** The largest table an exact comparison will build. Past it, nothing is claimed. */
const MAX_CELLS = 2_000_000;

/**
 * Which lines changed, by longest common subsequence over the trimmed middle.
 *
 * The same shape as the server's preview diff and for the same reason: the usual edit is
 * a few lines inside a file whose head and tail are identical, so trimming first makes
 * the table tiny. A table that would still be huge is refused rather than approximated.
 */
export function lineChanges(base: string | null, current: string): Changes {
  if (base === null) {
    return { added: new Set(), removedAfter: new Set(), removedCount: 0, coarse: false, untracked: true };
  }
  const old = base.split("\n");
  const now = current.split("\n");

  let head = 0;
  while (head < old.length && head < now.length && old[head] === now[head]) head += 1;
  let tail = 0;
  while (
    tail < old.length - head &&
    tail < now.length - head &&
    old[old.length - 1 - tail] === now[now.length - 1 - tail]
  ) {
    tail += 1;
  }
  const oldMiddle = old.slice(head, old.length - tail);
  const nowMiddle = now.slice(head, now.length - tail);

  const added = new Set<number>();
  const removedAfter = new Set<number>();
  let removedCount = 0;
  if (oldMiddle.length === 0 && nowMiddle.length === 0) {
    return { added, removedAfter, removedCount, coarse: false, untracked: false };
  }
  if (oldMiddle.length * nowMiddle.length > MAX_CELLS) {
    return { added, removedAfter, removedCount, coarse: true, untracked: false };
  }

  const columns = nowMiddle.length + 1;
  const table = new Uint32Array((oldMiddle.length + 1) * columns);
  for (let i = oldMiddle.length - 1; i >= 0; i -= 1) {
    for (let j = nowMiddle.length - 1; j >= 0; j -= 1) {
      table[i * columns + j] =
        oldMiddle[i] === nowMiddle[j]
          ? (table[(i + 1) * columns + j + 1] ?? 0) + 1
          : Math.max(table[(i + 1) * columns + j] ?? 0, table[i * columns + j + 1] ?? 0);
    }
  }

  let i = 0;
  let j = 0;
  // `line` is the one-based line in the current buffer the walk is standing on.
  const mark = (removal: boolean) => {
    const line = head + j;
    if (removal) {
      removedCount += 1;
      // Clamped: a removal above the first line has no line above it to hang under, and
      // the first line is the closest place that is not a lie about which line it was.
      removedAfter.add(Math.max(line, 1));
    } else {
      added.add(line + 1);
    }
  };
  while (i < oldMiddle.length && j < nowMiddle.length) {
    if (oldMiddle[i] === nowMiddle[j]) {
      i += 1;
      j += 1;
    } else if ((table[(i + 1) * columns + j] ?? 0) >= (table[i * columns + j + 1] ?? 0)) {
      mark(true);
      i += 1;
    } else {
      mark(false);
      j += 1;
    }
  }
  while (i < oldMiddle.length) {
    mark(true);
    i += 1;
  }
  while (j < nowMiddle.length) {
    mark(false);
    j += 1;
  }
  return { added, removedAfter, removedCount, coarse: false, untracked: false };
}

export const setChanges = StateEffect.define<Changes>();

const changesField = StateField.define<Changes>({
  create: () => NO_CHANGES,
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(setChanges)) return effect.value;
    return value;
  },
});

class ChangeMark extends GutterMarker {
  override elementClass: string;

  constructor(kind: "added" | "removed") {
    super();
    this.elementClass = `cm-synth-change-${kind}`;
  }
}

const addedMark = new ChangeMark("added");
const removedMark = new ChangeMark("removed");

/**
 * The 14px marker column, and the row tint behind an added line.
 *
 * An untracked file draws nothing: "every line is new" is true of a new file and says
 * nothing, and a solid cyan bar down a whole buffer only hides the real markers later.
 */
export function changeMarkers(): Extension {
  return [
    changesField,
    gutter({
      class: "cm-synth-change",
      lineMarker: (view, line) => {
        const changes = view.state.field(changesField, false);
        if (changes === undefined || changes.untracked || changes.coarse) return null;
        const number = view.state.doc.lineAt(line.from).number;
        if (changes.added.has(number)) return addedMark;
        if (changes.removedAfter.has(number)) return removedMark;
        return null;
      },
      initialSpacer: () => addedMark,
    }),
    EditorView.decorations.compute([changesField], (state) => {
      const changes = state.field(changesField, false);
      const builder = new RangeSetBuilder<Decoration>();
      if (changes === undefined || changes.untracked || changes.coarse) return builder.finish();
      const tint = Decoration.line({ class: "cm-synth-added" });
      for (const number of [...changes.added].sort((a, b) => a - b)) {
        if (number >= 1 && number <= state.doc.lines) {
          builder.add(state.doc.line(number).from, state.doc.line(number).from, tint);
        }
      }
      return builder.finish();
    }),
  ];
}
