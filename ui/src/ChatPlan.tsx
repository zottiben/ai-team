import { lazy, Suspense, useEffect, useId, useRef, useState } from "react";
import { BoardMarkdown } from "./BoardMarkdown";
import { PlanKanban } from "./PlanKanban";
import { PlanEditors, usePlanEditors } from "./PlanEditors";
import { PlanQuestions } from "./PlanQuestions";
import { chatPlan, changeChatPlan, PLAN_STATUSES } from "./plan-api";
import type {
  ChatPlan as Snapshot,
  PlanStatus,
} from "./plan-api";
import {
  CreatePlan,
  SliceStatus,
  statusLabel,
} from "./PlanForms";
import type { SavePlan } from "./PlanForms";
import "./chat-planning.css";
import "./synth/views/Board/board.css";
import { useSynth } from "./ui-preferences";

const SynthPlanHeader = lazy(() => import("./synth/views/Board/PlanHeader"));
const SynthPlanTabs = lazy(() => import("./synth/views/Board/PlanHeader").then(module => ({ default: module.PlanTabs })));

type Props = {
  chatId: number;
  tick: number;
  archived: boolean;
  frozen?: boolean;
  workspace?: boolean;
  onChanged: () => void;
};
export function ChatPlanning(props: Props) { return <PlanningController key={props.chatId} {...props}/>; }
function PlanningController({ chatId, tick, archived, frozen = false, workspace: libraryWorkspace = false, onChanged }: Props) {
  const synth = useSynth(), workspace = libraryWorkspace || synth, tabPrefix = useId();
  const boardOpened = useRef(workspace), synthOpened = useRef(synth);
  if (workspace) boardOpened.current = true;
  if (synth) synthOpened.current = true;
  const [view, setView] = useState<"board" | "plan" | "questions">("board");
  const [reveal, setReveal] = useState<{ key: string; nonce: number } | null>(null);
  const planVisible = view === "plan" || (!synth && view === "questions");
  const questionsVisible = !workspace || (synth ? view === "questions" : planVisible);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [loadProblem, setLoadProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const alive = useRef(true);
  const saving = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    let cancelled = false;
    void chatPlan(chatId)
      .then((next) => {
        if (!cancelled) {
          if (next.chat_id !== chatId) throw Error("Plan snapshot no longer matches this chat.");
          setSnapshot((previous) =>
            previous && previous.revision > next.revision ? previous : next,
          );
          setLoadProblem(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setLoadProblem(
            error instanceof Error ? error.message : String(error),
          );
      });
    return () => {
      cancelled = true;
    };
  }, [chatId, tick, reload]);

  const save: SavePlan = async (action, revision = snapshot?.revision ?? 0) => {
    if (saving.current || archived || snapshot?.archived) return false;
    if ((frozen || snapshot?.frozen) && action.action !== "answer_question" && action.action !== "open_question") return false;
    saving.current = true;
    setBusy(true);
    setProblem(null);
    try {
      const next = await changeChatPlan(chatId, action, revision);
      if (!alive.current) return false;
      if (next.chat_id !== chatId) throw Error("Plan response no longer matches this chat.");
      setSnapshot((previous) =>
        previous && previous.revision > next.revision ? previous : next,
      );
      onChanged();
      return true;
    } catch (error: unknown) {
      if (alive.current) {
        setProblem(error instanceof Error ? error.message : String(error));
        setReload((value) => value + 1);
      }
      return false;
    } finally {
      saving.current = false;
      if (alive.current) setBusy(false);
    }
  };
  const issue = problem ?? loadProblem;
  const readOnly = archived || snapshot?.archived === true;
  const locked = frozen || snapshot?.frozen === true;
  const bundle = snapshot?.bundle;
  const editors = usePlanEditors(bundle, snapshot?.revision ?? 0);
  const done =
    bundle?.slices.filter((slice) => slice.status === "done").length ?? 0;
  return (
    <section
      className="chat-overview-card chat-planning"
      aria-label="Chat plan"
    >
      {!workspace && (
        <div className="plan-heading">
          <h3>Plan</h3>
          <span className="faint">Only this chat · built in</span>
        </div>
      )}
      <details hidden={workspace && !planVisible}>
        <summary>Planning isolation</summary>
        <p className="faint">
          This chat uses AI Team’s scoped planning tools and permitted project
          MCP servers, not global MCP discovery. Standalone ai-planner, global
          registrations and credentials are left unchanged. Tool scoping is not
          an OS sandbox.
        </p>
      </details>
      {issue && (
        <div className="error" role="alert">
          {issue}{" "}
          <button
            className="button"
            onClick={() => {
              setProblem(null);
              setLoadProblem(null);
              setReload((value) => value + 1);
            }}
          >
            Refresh plan
          </button>
        </div>
      )}
      {!snapshot ? (
        <p className="faint">
          {issue ? "The plan could not be loaded." : "Loading plan…"}
        </p>
      ) : (
        <fieldset
          disabled={busy}
          className="plan-controls"
        >
          {!bundle ? (
            <>
              <p className="faint">
                No plan yet. Chat normally, ask Pi to plan, or create one here.
                Nothing is written to standalone ai-planner.
              </p>
              {!readOnly && <CreatePlan save={save} />}
            </>
          ) : (
            <>
              <div className="plan-head">
              <div className="plan-heading">
                {synth ? <h2>{bundle.plan.title}</h2> : <h4>{bundle.plan.title}</h4>}
                <label>
                  Plan status
                  <select
                    disabled={locked || readOnly}
                    value={bundle.plan.status}
                    onChange={(event) =>
                      void save({
                        action: "set_plan_status",
                        status: event.target.value as PlanStatus,
                      })
                    }
                  >
                    {PLAN_STATUSES.map((status) => (
                      <option value={status} key={status}>
                        {statusLabel(status)}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              <div hidden={!synth}>{synthOpened.current && <Suspense fallback={<p className="faint">Loading slice pattern…</p>}><SynthPlanHeader slices={bundle.slices} onReveal={key => {
                setView("board"); setReveal(previous => ({ key, nonce: (previous?.nonce ?? 0) + 1 }));
              }}/></Suspense>}</div>
              </div>
              {bundle.plan.summary && (
                <div hidden={workspace && !planVisible}>
                  <BoardMarkdown source={bundle.plan.summary} />
                </div>
              )}
              <div className="plan-progress" hidden={synth}>
                <span>
                  {done} of {bundle.slices.length} slices done
                </span>
                {bundle.slices.length > 0 && (
                  <progress
                    aria-label="Reported slice progress"
                    value={done}
                    max={bundle.slices.length}
                  />
                )}
                {!workspace && (
                  <small className="faint">
                    Reported progress, not automatic verification or execution.
                  </small>
                )}
              </div>
              {boardOpened.current && (
                <>
                  <nav className="plan-workspace-tabs" aria-label={synth ? undefined : "Plan views"} hidden={!workspace}>
                    {synth ? <Suspense fallback={<span className="faint">Loading plan views…</span>}><SynthPlanTabs prefix={tabPrefix} view={view} questions={bundle.questions.filter(question => question.status === "open").length} onChange={setView}/></Suspense> : <>
                    <button
                      type="button"
                      aria-current={view === "board" ? "page" : undefined}
                      onClick={() => setView("board")}
                    >
                      Board
                    </button>
                    <button
                      type="button"
                      aria-current={planVisible ? "page" : undefined}
                      onClick={() => setView("plan")}
                    >
                      Plan
                    </button>
                    {bundle.questions.some(
                      (question) => question.status === "open",
                    ) && (
                      <button type="button" onClick={() => setView("plan")}>
                        {
                          bundle.questions.filter(
                            (question) => question.status === "open",
                          ).length
                        }{" "}
                        open questions
                      </button>
                    )}</>}
                  </nav>
                  {workspace && locked && view === "board" && (
                    <p className="notice">
                      Approved work is frozen until this build releases the
                      chat. Questions remain available; answers do not approve more work.
                    </p>
                  )}
                  {workspace && readOnly && (
                    <p className="notice">
                      This chat is archived. Its plan and board remain readable;
                      restore the chat to edit.
                    </p>
                  )}
                  <div id={`${tabPrefix}-panel-board`} role={synth ? "tabpanel" : undefined} aria-labelledby={synth ? `${tabPrefix}-board` : undefined} hidden={!workspace || view !== "board"}>
                    <PlanKanban
                      slices={bundle.slices}
                      reveal={reveal}
                      revision={snapshot.revision}
                      save={save}
                      disabled={busy || locked || readOnly}
                    />
                  </div>
                </>
              )}
              <div id={`${tabPrefix}-panel-questions`} role={synth ? "tabpanel" : undefined} aria-labelledby={synth ? `${tabPrefix}-questions` : undefined} hidden={!questionsVisible}>
                <PlanQuestions questions={bundle.questions} revision={snapshot.revision} save={save} readOnly={readOnly} />
              </div>
              <div id={`${tabPrefix}-panel-plan`} role={synth ? "tabpanel" : undefined} aria-labelledby={synth ? `${tabPrefix}-plan` : undefined} hidden={workspace && !planVisible}>
                {locked && (
                  <p className="notice">
                    Approved work is frozen until this build releases the chat.
                    Questions remain available; answers do not approve more
                    work.
                  </p>
                )}
                <fieldset
                  disabled={locked || readOnly}
                  className="plan-controls"
                >
                  <div className="plan-heading">
                    <h4>Scope & notes</h4>
                    <button
                      className="button"
                      onClick={() => editors.open("section")}
                    >
                      Add section
                    </button>
                  </div>
                  {bundle.sections
                    .filter((section) => section.body.trim())
                    .map((section) => (
                      <article className="plan-item" key={section.key}>
                        <div className="plan-heading">
                          <h5>{section.title}</h5>
                          <button
                            className="button"
                            onClick={() =>
                              editors.open("section", section.key)
                            }
                          >
                            Edit {section.title}
                          </button>
                        </div>
                        <BoardMarkdown source={section.body} />
                      </article>
                    ))}
                  <PlanEditors kind="section" editors={editors} revision={snapshot.revision} save={save}/>
                  <div className="plan-heading">
                    <h4>Work slices</h4>
                    <button
                      className="button"
                      onClick={() => editors.open("slice")}
                    >
                      Add slice
                    </button>
                  </div>
                  {!bundle.slices.length && (
                    <p className="faint">
                      Break the work into verifiable slices when useful.
                    </p>
                  )}
                  {bundle.slices.map((slice) => (
                    <article className="plan-item" key={slice.id}>
                      <div className="plan-heading">
                        <h5>
                          {slice.key} · {slice.title}
                        </h5>
                        <button
                          className="button"
                          onClick={() =>
                            editors.open("slice", slice.key)
                          }
                        >
                          Edit {slice.key}
                        </button>
                      </div>
                      <BoardMarkdown source={slice.scope_md} />
                      {slice.demo_md && (
                        <details>
                          <summary>Verification criteria</summary>
                          <BoardMarkdown source={slice.demo_md} />
                        </details>
                      )}
                      {slice.blocked_reason && (
                        <p className="notice">
                          Blocked: {slice.blocked_reason}
                        </p>
                      )}
                      <SliceStatus slice={slice} revision={snapshot.revision} save={save} />
                    </article>
                  ))}
                  <PlanEditors kind="slice" editors={editors} revision={snapshot.revision} save={save}/>
                </fieldset>
                {bundle.decisions.length > 0 && (
                  <details>
                    <summary>Decisions ({bundle.decisions.length})</summary>
                    {bundle.decisions.map((decision) => (
                      <article className="plan-item" key={decision.id}>
                        <h5>
                          {decision.key} · {decision.title} · {decision.status}
                        </h5>
                        <BoardMarkdown source={decision.body} />
                      </article>
                    ))}
                  </details>
                )}
                {bundle.gotchas.length > 0 && (
                  <details>
                    <summary>Gotchas ({bundle.gotchas.length})</summary>
                    {bundle.gotchas.map((gotcha) => (
                      <article className="plan-item" key={gotcha.id}>
                        <h5>{gotcha.title}</h5>
                        <BoardMarkdown source={gotcha.body} />
                      </article>
                    ))}
                  </details>
                )}
                {bundle.log.length > 0 && (
                  <details>
                    <summary>Planning activity ({bundle.log.length})</summary>
                    <ol className="plan-log">
                      {bundle.log
                        .slice()
                        .reverse()
                        .map((entry) => (
                          <li key={entry.id}>
                            <BoardMarkdown source={entry.body} />
                            <small className="faint">
                              {entry.actor ?? "Planner"} ·{" "}
                              {new Date(entry.at).toLocaleString()}
                            </small>
                          </li>
                        ))}
                    </ol>
                  </details>
                )}
              </div>
            </>
          )}
        </fieldset>
      )}
    </section>
  );
}
