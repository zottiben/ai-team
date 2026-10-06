import { useState } from "react";
import { BoardMarkdown } from "./BoardMarkdown";
import { PLAN_STATUSES, type PlanSlice, type PlanStatus } from "./plan-api";
import {
  SliceForm,
  SliceStatus,
  statusLabel,
  type SavePlan,
} from "./PlanForms";

export function PlanKanban({
  slices,
  revision,
  save,
  disabled,
}: {
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
  const open = (key: string) => {
    setSelected(key);
    setOpened((previous) =>
      previous.includes(key) ? previous : [...previous, key],
    );
  };
  return (
    <div>
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
        <div className="plan-kanban" role="region" aria-label="Plan board">
          {PLAN_STATUSES.map((status) => {
            const lane = slices.filter((slice) => slice.status === status);
            return (
              <section
                key={status}
                className={`plan-lane${collapsed[status] ? " is-collapsed" : ""}`}
                data-status={status}
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
                    <span>{lane.length}</span>
                    <span aria-hidden="true">
                      {collapsed[status] ? "›" : "‹"}
                    </span>
                  </button>
                </header>
                {!collapsed[status] && (
                  <div className="plan-lane-cards">
                    {lane.length === 0 ? (
                      <p className="faint">Nothing here.</p>
                    ) : (
                      lane.map((slice) => (
                        <article
                          key={slice.id}
                          className="plan-slice-card"
                          aria-current={
                            selected === slice.key ? "true" : undefined
                          }
                        >
                          <button
                            type="button"
                            className="plan-slice-open"
                            aria-label={`${slice.key} ${slice.title}`}
                            onClick={() => open(slice.key)}
                          >
                            <span className="mono faint">{slice.key}</span>
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
                            <p className="notice">{slice.blocked_reason}</p>
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
    </div>
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
        <SliceStatus slice={slice} save={save} />
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
