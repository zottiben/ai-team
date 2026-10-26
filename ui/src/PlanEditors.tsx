import { useState } from "react";
import { SectionForm, SliceForm, type SavePlan } from "./PlanForms";
import type { ChatPlan, PlanSection, PlanSlice } from "./plan-api";

type Editor = { id: string; revision: number } & (
  | { kind: "section"; section?: PlanSection }
  | { kind: "slice"; slice?: PlanSlice }
);
type Editors = { active: string | null; entries: Editor[] };

/** A manually opened form owns its initial fields and revision until explicit close. */
export function usePlanEditors(bundle: ChatPlan["bundle"] | undefined, revision: number) {
  const [state, setState] = useState<Editors>({ active: null, entries: [] });
  const open = (kind: Editor["kind"], key?: string) => {
    if (!bundle) return;
    const section = bundle.sections.find(value => value.key === key);
    const slice = bundle.slices.find(value => value.key === key);
    if (key !== undefined && !(kind === "section" ? section : slice)) return;
    const id = JSON.stringify([kind, key ?? null]);
    const entry: Editor = kind === "section" ? { id, kind, revision, section } : { id, kind, revision, slice };
    setState(previous => ({ active: id, entries: previous.entries.some(value => value.id === id) ? previous.entries : [...previous.entries, entry] }));
  };
  const select = (id: string) => setState(previous => previous.entries.some(entry => entry.id === id) ? { ...previous, active: id } : previous);
  const close = (id: string) => setState(previous => ({ active: previous.active === id ? null : previous.active, entries: previous.entries.filter(entry => entry.id !== id) }));
  return { state, open, select, close };
}

export function PlanEditors({ kind, editors, revision, save }: {
  kind: Editor["kind"]; editors: ReturnType<typeof usePlanEditors>; revision: number; save: SavePlan;
}) {
  const entries = editors.state.entries.filter(entry => entry.kind === kind);
  if (!entries.length) return null;
  return <div className="plan-editors">
    <nav className="plan-actions" aria-label={`Retained ${kind} editors`}>
      {entries.filter(entry => entry.id !== editors.state.active).map(entry => <button type="button" className="button" key={entry.id} onClick={() => editors.select(entry.id)}>
        Return to {kind} {entry.kind === "section" ? entry.section?.key ?? "new section" : entry.slice?.key ?? "new slice"}
      </button>)}
    </nav>
    {entries.map(entry => <div key={entry.id} hidden={editors.state.active !== entry.id}>
      {entry.kind === "section"
        ? <SectionForm section={entry.section} revision={revision} initialRevision={entry.revision} save={save} close={() => editors.close(entry.id)}/>
        : <SliceForm slice={entry.slice} revision={revision} initialRevision={entry.revision} save={save} close={() => editors.close(entry.id)}/>}
    </div>)}
  </div>;
}
