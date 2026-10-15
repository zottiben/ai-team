import { PLAN_STATUSES, type PlanSlice, type PlanStatus } from "../../../plan-api";

export type StatusDrop = { key: string; from: PlanStatus; to: PlanStatus; revision: number };
export type DropView = { slices: readonly PlanSlice[]; revision: number; disabled: boolean };

/** A gesture proposes one status at one revision, never an ordering or a run. */
export function currentDrop(view: DropView, drop: StatusDrop): boolean {
  const matches = view.slices.filter(slice => slice.key === drop.key);
  return !view.disabled && Number.isSafeInteger(drop.revision) && drop.revision >= 0
    && view.revision === drop.revision && PLAN_STATUSES.includes(drop.to)
    && matches.length === 1 && matches[0]!.status === drop.from && drop.from !== drop.to;
}

/** Display-only position derived from the authoritative plan order. */
export function statusPosition(slices: readonly PlanSlice[], key: string, status: PlanStatus): number | null {
  const index = slices.findIndex(slice => slice.key === key);
  return index < 0 ? null : slices.slice(0, index).filter(slice => slice.status === status).length + 1;
}
