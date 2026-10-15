import type { ReactNode } from "react";
import type { PlanSlice } from "../../../plan-api";
import { SlicePattern } from "./SlicePattern";
import { TabStrip } from "../../primitives/TabStrip";

export type PlanView = "board" | "plan" | "questions";
export function PlanTabs({ prefix, view, questions, onChange }: { prefix: string; view: PlanView; questions: number; onChange: (view: PlanView) => void }) {
  const tabs = [
    { id: `${prefix}-board`, panel: `${prefix}-panel-board`, label: "Board" },
    { id: `${prefix}-plan`, panel: `${prefix}-panel-plan`, label: "Plan" },
    { id: `${prefix}-questions`, panel: `${prefix}-panel-questions`, label: "Open questions", badge: questions || undefined },
  ];
  return <TabStrip label="Plan views" tabs={tabs} value={`${prefix}-${view}`} onChange={id => {
    const next = (["board", "plan", "questions"] as const).find(view => `${prefix}-${view}` === id);
    if (next) onChange(next);
  }}/>;
}

/** Derived planning facts only: no inferred ticket, execution, throughput or WIP limit. */
export default function PlanHeader({ slices, onReveal, statusControl }: { slices: PlanSlice[]; onReveal: (key: string) => void; statusControl?: ReactNode }) {
  const count = (status: PlanSlice["status"]) => slices.filter(slice => slice.status === status).length;
  const done = count("done"), review = count("in_review"), blocked = count("blocked");
  return <div className="synth-plan-overview">
    <div className="synth-plan-overview__pattern">
      <p>{done} of {slices.length} slices done · {review} in review · {blocked} blocked</p>
      <SlicePattern slices={slices} onReveal={onReveal}/>
    </div>
    <div className="synth-plan-overview__count">
      <span className="synth-silk">Reported done</span>
      <output aria-label="Reported slice count" aria-live="off">{done}<span>/{slices.length}</span></output>
      <span>Not verification</span>
      {statusControl}
    </div>
  </div>;
}
