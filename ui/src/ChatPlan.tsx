import { useEffect, useRef, useState } from "react";
import { BoardMarkdown } from "./BoardMarkdown";
import { PlanKanban } from "./PlanKanban";
import { chatPlan, changeChatPlan, PLAN_STATUSES } from "./plan-api";
import type {
  ChatPlan as Snapshot,
  PlanQuestion,
  PlanStatus,
} from "./plan-api";
import {
  CreatePlan,
  SectionForm,
  SliceForm,
  SliceStatus,
  statusLabel,
} from "./PlanForms";
import type { SavePlan } from "./PlanForms";
import "./chat-planning.css";

export function ChatPlanning({
  chatId,
  tick,
  archived,
  frozen = false,
  workspace = false,
  onChanged,
}: {
  chatId: number;
  tick: number;
  archived: boolean;
  frozen?: boolean;
  workspace?: boolean;
  onChanged: () => void;
}) {
  const [view, setView] = useState<"board" | "plan">("board");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [loadProblem, setLoadProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const [editor, setEditor] = useState<{
    kind: "section" | "slice";
    key?: string;
  } | null>(null);
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
    if (saving.current) return false;
    saving.current = true;
    setBusy(true);
    setProblem(null);
    try {
      const next = await changeChatPlan(chatId, action, revision);
      if (!alive.current) return false;
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
      <details hidden={workspace && view !== "plan"}>
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
          disabled={busy || (readOnly && !workspace)}
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
              <div className="plan-heading">
                <h4>{bundle.plan.title}</h4>
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
              {bundle.plan.summary && (
                <div hidden={workspace && view === "board"}>
                  <BoardMarkdown source={bundle.plan.summary} />
                </div>
              )}
              <div className="plan-progress">
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
              {workspace && (
                <>
                  <nav className="plan-workspace-tabs" aria-label="Plan views">
                    <button
                      type="button"
                      aria-current={view === "board" ? "page" : undefined}
                      onClick={() => setView("board")}
                    >
                      Board
                    </button>
                    <button
                      type="button"
                      aria-current={view === "plan" ? "page" : undefined}
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
                    )}
                  </nav>
                  {locked && view === "board" && (
                    <p className="notice">
                      Approved work is frozen until this build releases the
                      chat. Open Plan to read or answer questions.
                    </p>
                  )}
                  {readOnly && (
                    <p className="notice">
                      This chat is archived. Its plan and board remain readable;
                      restore the chat to edit.
                    </p>
                  )}
                  <div hidden={view !== "board"}>
                    <PlanKanban
                      slices={bundle.slices}
                      revision={snapshot.revision}
                      save={save}
                      disabled={busy || locked || readOnly}
                    />
                  </div>
                </>
              )}
              <div hidden={workspace && view !== "plan"}>
                <fieldset disabled={readOnly} className="plan-controls">
                  <PlanQuestions questions={bundle.questions} save={save} />
                </fieldset>
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
                      onClick={() => setEditor({ kind: "section" })}
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
                              setEditor({ kind: "section", key: section.key })
                            }
                          >
                            Edit {section.title}
                          </button>
                        </div>
                        <BoardMarkdown source={section.body} />
                      </article>
                    ))}
                  {editor?.kind === "section" && (
                    <SectionForm
                      key={`section-${editor.key ?? "new"}`}
                      section={bundle.sections.find(
                        (section) => section.key === editor.key,
                      )}
                      revision={snapshot.revision}
                      save={save}
                      close={() => setEditor(null)}
                    />
                  )}
                  <div className="plan-heading">
                    <h4>Work slices</h4>
                    <button
                      className="button"
                      onClick={() => setEditor({ kind: "slice" })}
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
                            setEditor({ kind: "slice", key: slice.key })
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
                      <SliceStatus slice={slice} save={save} />
                    </article>
                  ))}
                  {editor?.kind === "slice" && (
                    <SliceForm
                      key={`slice-${editor.key ?? "new"}`}
                      slice={bundle.slices.find(
                        (slice) => slice.key === editor.key,
                      )}
                      revision={snapshot.revision}
                      save={save}
                      close={() => setEditor(null)}
                    />
                  )}
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

function PlanQuestions({
  questions,
  save,
}: {
  questions: PlanQuestion[];
  save: SavePlan;
}) {
  const [asking, setAsking] = useState(false);
  const [body, setBody] = useState("");
  const open = questions.filter((question) => question.status === "open");
  const resolved = questions.filter((question) => question.status !== "open");
  return (
    <section className="plan-questions" aria-label="Plan questions">
      <div className="plan-heading">
        <h4>Questions {open.length > 0 && `(${open.length} open)`}</h4>
        <button className="button" onClick={() => setAsking(!asking)}>
          {asking ? "Cancel question" : "Ask a question"}
        </button>
      </div>
      <p className="faint">
        Human decisions for this plan. An answer does not approve a tool, start
        a run, or resume a stopped turn.
      </p>
      {asking && (
        <form
          className="plan-form"
          onSubmit={(event) => {
            event.preventDefault();
            void save({ action: "open_question", body }).then((ok) => {
              if (ok) {
                setBody("");
                setAsking(false);
              }
            });
          }}
        >
          <label>
            Question
            <textarea
              value={body}
              onChange={(event) => setBody(event.target.value)}
              required
              maxLength={16000}
            />
          </label>
          <div>
            <button className="button">Save question</button>
          </div>
        </form>
      )}
      {open.map((question) => (
        <Question key={question.id} question={question} save={save} />
      ))}
      {resolved.length > 0 && (
        <details>
          <summary>Answered questions ({resolved.length})</summary>
          {resolved.map((question) => (
            <article className="plan-item" key={question.id}>
              <BoardMarkdown source={question.body} />
              {question.answer && <BoardMarkdown source={question.answer} />}
            </article>
          ))}
        </details>
      )}
    </section>
  );
}

function Question({
  question,
  save,
}: {
  question: PlanQuestion;
  save: SavePlan;
}) {
  const [answer, setAnswer] = useState("");
  return (
    <article className="plan-item">
      {question.slice_key && (
        <span className="faint">{question.slice_key}</span>
      )}
      <BoardMarkdown source={question.body} />
      <form
        className="plan-form"
        onSubmit={(event) => {
          event.preventDefault();
          void save({
            action: "answer_question",
            question_id: question.id,
            answer,
          });
        }}
      >
        <label>
          Your answer
          <textarea
            value={answer}
            onChange={(event) => setAnswer(event.target.value)}
            required
            maxLength={16000}
            rows={2}
          />
        </label>
        <div>
          <button className="button">Answer question</button>
        </div>
      </form>
    </article>
  );
}
