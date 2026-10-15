import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useSynth } from "./ui-preferences";
import { SynthProvider } from "./synth/SynthProvider";
import { useMotionVisible } from "./synth/motion/useMotionVisible";
import { Led } from "./synth/primitives/Led";
import { SegmentMeter } from "./synth/primitives/SegmentMeter";
import { signalStyle } from "./synth/primitives/signal";
import { statusSignals } from "./synth/views/Board/status";
import { BoardMarkdown } from "./BoardMarkdown";
import { PLAN_STATUSES, type PlanSlice, type PlanStatus } from "./plan-api";
import {
  SliceForm,
  SliceStatus,
  statusLabel,
  type SavePlan,
} from "./PlanForms";
import "./plan-library.css";

export function PlanKanban({
  slices,
  revision,
  save,
  disabled,
  reveal = null,
}: {
  reveal?: { key: string; nonce: number } | null;
  slices: PlanSlice[];
  revision: number;
  save: SavePlan;
  disabled: boolean;
}) {
  const [collapsed, setCollapsed] = useState<
    Partial<Record<PlanStatus, boolean>>
  >({ draft: true });
  const [selected, setSelected] = useState<string | null>(null);
  const [opened, setOpened] = useState<string[]>([]);
  const [adding, setAdding] = useState(false);
  const synth = useSynth(), host = useRef<HTMLDivElement>(null), motion = useMotionVisible(host);
  const handled = useRef<typeof reveal>(null);
  const [revealed, setRevealed] = useState<string | null>(null);
  useLayoutEffect(() => {
    if (!reveal || handled.current === reveal) return;
    handled.current = reveal;
    const slice = slices.find(value => value.key === reveal.key), owner = document.activeElement;
    if (!synth || !slice || document.hidden) return;
    const focus = () => {
      cancel();
      const button = [...(host.current?.querySelectorAll<HTMLButtonElement>("[data-slice-open]") ?? [])].find(node => node.dataset.sliceOpen === slice.key);
      const released = document.activeElement === document.body && owner instanceof HTMLElement && !owner.getClientRects().length;
      if (document.hidden || !button?.getClientRects().length || (document.activeElement !== owner && !released)) return;
      button.scrollIntoView({ block: "nearest", inline: "center" }); button.focus({ preventScroll: true });
      setRevealed(slice.key);
    };
    setCollapsed(previous => previous[slice.status] ? { ...previous, [slice.status]: false } : previous);
    const timer = window.setTimeout(focus, collapsed[slice.status] && motion ? 240 : 0);
    function cancel() { clearTimeout(timer); document.removeEventListener("pointerdown", cancel); document.removeEventListener("visibilitychange", visibility); }
    function visibility() { if (document.hidden) cancel(); }
    document.addEventListener("pointerdown", cancel);
    document.addEventListener("visibilitychange", visibility);
    return cancel;
    // Each request is consumed once. A revision or skin change cancels its deferred
    // focus; expanding its lane must not restart the request or steal newer focus.
  }, [reveal, synth, revision]);
  useEffect(() => {
    if (!revealed) return;
    if (!motion) { setRevealed(null); return; }
    const timer = setTimeout(() => setRevealed(null), 180);
    return () => clearTimeout(timer);
  }, [revealed, motion]);
  const open = (key: string) => {
    setSelected(key);
    setOpened((previous) =>
      previous.includes(key) ? previous : [...previous, key],
    );
  };
  return (
    <SynthProvider><div>
      <div className="plan-heading">
        <p className="faint">
          Status only · not verification or execution approval.
        </p>
        <button
          type="button"
          className="button"
          disabled={disabled}
          onClick={() => setAdding(true)}
        >
          Add slice
        </button>
      </div>
      {adding && (
        <fieldset className="plan-controls" disabled={disabled}>
          <SliceForm
            revision={revision}
            save={save}
            close={() => setAdding(false)}
          />
        </fieldset>
      )}
      <div className={`plan-board-shell${selected ? " has-drawer" : ""}`}>
        <div ref={host} className="plan-kanban" role="region" aria-label="Plan board" data-motion={synth && motion}>
          {PLAN_STATUSES.map((status) => {
            const lane = slices.filter((slice) => slice.status === status);
            return (
              <section
                key={status}
                className={`plan-lane${collapsed[status] ? " is-collapsed" : ""}`}
                data-status={status}
                style={synth ? signalStyle(statusSignals[status]) : undefined}
                aria-label={`${statusLabel(status)} slices`}
              >
                <header>
                  <button
                    type="button"
                    aria-label={`${collapsed[status] ? "Expand" : "Collapse"} ${statusLabel(status)} lane`}
                    aria-expanded={!collapsed[status]}
                    onClick={() =>
                      setCollapsed((previous) => ({
                        ...previous,
                        [status]: !previous[status],
                      }))
                    }
                  >
                    <span className="plan-lane-dot" aria-hidden="true" />
                    <strong>{statusLabel(status)}</strong>
                    <span className="plan-lane-count">{lane.length}</span>
                    <span aria-hidden="true">
                      {collapsed[status] ? "›" : "‹"}
                    </span>
                  </button>
                </header>
                {synth && <SegmentMeter label="Slices · capped meter, not a limit" value={lane.length} segments={8} signal={statusSignals[status]}/>}
                {!collapsed[status] && (
                  <div className="plan-lane-cards">
                    {lane.length === 0 ? (
                      <p className="faint plan-lane-empty">{synth && status === "active" && <span className="synth-empty-jack" aria-hidden="true"/>}{synth && status === "active" ? "No slices marked active. Status is not execution." : "Nothing here."}</p>
                    ) : (
                      lane.map((slice) => (
                        <article
                          key={slice.id}
                          className="plan-slice-card"
                          data-revealed={revealed === slice.key || undefined}
                          aria-current={
                            selected === slice.key ? "true" : undefined
                          }
                        >
                          <button
                            type="button"
                            className="plan-slice-open"
                            data-slice-open={slice.key}
                            aria-label={`${slice.key} ${slice.title}`}
                            onClick={() => open(slice.key)}
                          >
                            {synth ? <span className="plan-slice-key"><span className="mono">{slice.key}</span><span className="plan-slice-state" aria-hidden="true"/></span> : <span className="mono faint">{slice.key}</span>}
                            <strong>{slice.title}</strong>
                          </button>
                          {slice.branch && (
                            <span className="faint mono" title={slice.branch}>
                              {slice.branch}
                            </span>
                          )}
                          {slice.claimed_by && (
                            <span className="faint">
                              Recorded claim: {slice.claimed_by}
                            </span>
                          )}
                          {slice.worktree_path && (
                            <span
                              className="faint mono"
                              title={slice.worktree_path}
                            >
                              {slice.worktree_path}
                            </span>
                          )}
                          {slice.estimate_files != null && (
                            <span className="faint">
                              ~{slice.estimate_files} files
                            </span>
                          )}
                          {slice.pr_url &&
                            /^https?:\/\//.test(slice.pr_url) && (
                              <a
                                href={slice.pr_url}
                                target="_blank"
                                rel="noreferrer"
                              >
                                Pull request ↗
                              </a>
                            )}
                          {slice.blocked_reason && (
                            <p className="notice">{synth && <span className="synth-plan-blocker-label"><Led state="attention" size={7}/>Blocker · needs you</span>}{slice.blocked_reason}</p>
                          )}
                        </article>
                      ))
                    )}
                  </div>
                )}
              </section>
            );
          })}
        </div>
        <div className="plan-slice-drawers" hidden={!selected}>
          {opened.map((key) => {
            const slice = slices.find((slice) => slice.key === key);
            return slice ? (
              <div key={key} hidden={selected !== key}>
                <SliceDrawer
                  slice={slice}
                  revision={revision}
                  save={save}
                  disabled={disabled}
                  close={() => setSelected(null)}
                />
              </div>
            ) : null;
          })}
        </div>
      </div>
    </div></SynthProvider>
  );
}
function SliceDrawer({
  slice,
  revision,
  save,
  disabled,
  close,
}: {
  slice: PlanSlice;
  revision: number;
  save: SavePlan;
  disabled: boolean;
  close: () => void;
}) {
  const [editing, setEditing] = useState(false);
  return (
    <aside className="plan-slice-drawer" aria-label={`Slice ${slice.key}`}>
      <div className="plan-heading">
        <h4>
          {slice.key} · {slice.title}
        </h4>
        <button type="button" className="button" onClick={close}>
          Close slice
        </button>
      </div>
      <BoardMarkdown source={slice.scope_md} />
      {slice.demo_md && (
        <>
          <h5>Verification criteria</h5>
          <BoardMarkdown source={slice.demo_md} />
        </>
      )}
      <fieldset className="plan-controls" disabled={disabled}>
        <SliceStatus slice={slice} revision={revision} save={save} />
        {editing ? (
          <SliceForm
            slice={slice}
            revision={revision}
            save={save}
            close={() => setEditing(false)}
          />
        ) : (
          <button
            type="button"
            className="button"
            onClick={() => setEditing(true)}
          >
            Edit {slice.key}
          </button>
        )}
      </fieldset>
    </aside>
  );
}
