import { useCallback, useEffect, useState } from "react";
import { BoardMarkdown } from "./BoardMarkdown";
import type { SavePlan } from "./PlanForms";
import type { PlanQuestion } from "./plan-api";

type Draft = { body: string; revision: number };
export function PlanQuestions({ questions, revision, save, readOnly = false }: { questions: PlanQuestion[]; revision: number; save: SavePlan; readOnly?: boolean }) {
  const [compose, setCompose] = useState<{ asking: boolean; draft: Draft | null }>({ asking: false, draft: null });
  const [expanded, setExpanded] = useState(false);
  const [retained, setRetained] = useState<Record<number, PlanQuestion>>({});
  const retain = useCallback((id: number, question: PlanQuestion | null) => setRetained(previous => {
    if (previous[id] === question || (!question && !previous[id])) return previous;
    const next = { ...previous };
    if (question) next[id] = question; else delete next[id];
    return next;
  }), []);
  const open = questions.filter(question => question.status === "open");
  const resolved = questions.filter(question => question.status !== "open");
  const ids = new Set(questions.map(question => question.id));
  const shown = [...questions, ...Object.values(retained).filter(question => !ids.has(question.id))];
  const { asking, draft } = compose;
  return <section className="plan-questions" aria-label="Plan questions">
    <div className="plan-heading">
      <h4>Questions {open.length > 0 && `(${open.length} open)`}</h4>
      <button type="button" className="button" disabled={readOnly && !asking && !draft} onClick={() => setCompose(previous => ({ ...previous, asking: !previous.asking }))}>
        {asking ? "Cancel question" : "Ask a question"}
      </button>
    </div>
    <p className="faint">Human decisions for this plan. An answer does not approve a tool, start a run, or resume a stopped turn.</p>
    {(asking || draft) && <form className="plan-form" hidden={!asking} onSubmit={event => {
      event.preventDefault();
      if (readOnly || !draft?.body.trim() || draft.revision !== revision) return;
      void save({ action: "open_question", body: draft.body }, draft.revision).then(ok => {
        if (ok) setCompose(current => current.draft === draft ? { asking: false, draft: null } : current);
      });
    }}>
      <label>Question<textarea value={draft?.body ?? ""} readOnly={readOnly} required maxLength={16000} onChange={event => {
        if (readOnly) return;
        const body = event.target.value;
        setCompose(previous => ({ ...previous, draft: body ? { body, revision: previous.draft?.revision ?? revision } : null }));
      }}/></label>
      {draft && draft.revision !== revision && <p className="notice">The plan changed after this question was started. The draft is retained against revision {draft.revision}.
        <button type="button" className="button" disabled={readOnly} onClick={() => setCompose(current => current.draft === draft ? { ...current, draft: { ...draft, revision } } : current)}>Keep question for current revision</button>
      </p>}
      <div><button className="button" disabled={readOnly || !draft?.body.trim() || draft.revision !== revision}>Save question</button></div>
    </form>}
    {draft && !asking && <p className="faint">Unsent question retained.</p>}
    {resolved.length > 0 && <button type="button" className="button" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>Answered questions ({resolved.length})</button>}
    {shown.map(question => <Question key={question.id} question={question} revision={revision} present={ids.has(question.id)} expanded={expanded} save={save} retain={retain} readOnly={readOnly}/>)}
  </section>;
}

function Question({ question, revision, present, expanded, save, retain, readOnly }: {
  question: PlanQuestion; revision: number; present: boolean; expanded: boolean; save: SavePlan; readOnly: boolean;
  retain: (id: number, question: PlanQuestion | null) => void;
}) {
  const [draft, setDraft] = useState<(Draft & { question: PlanQuestion }) | null>(null);
  const captured = draft?.question ?? null;
  useEffect(() => retain(question.id, captured), [question.id, captured, retain]);
  const open = present && question.status === "open";
  const fresh = open && (!draft || (draft.revision === revision && draft.question.body === question.body));
  return <article className="plan-item" hidden={!open && !draft && !expanded}>
    {question.slice_key && <span className="faint">{question.slice_key}</span>}
    <BoardMarkdown source={question.body}/>
    {question.answer && <><span className="faint">Recorded answer</span><BoardMarkdown source={question.answer}/></>}
    {!present && <p className="notice">This question is no longer present. Its unsent answer is retained and cannot be submitted.</p>}
    {(open || draft) && <form className="plan-form" onSubmit={event => {
      event.preventDefault();
      if (readOnly || !draft?.body.trim() || !fresh) return;
      void save({ action: "answer_question", question_id: draft.question.id, answer: draft.body }, draft.revision).then(ok => {
        if (ok) setDraft(current => current === draft ? null : current);
      });
    }}>
      <label>Your answer<textarea value={draft?.body ?? ""} readOnly={readOnly} required maxLength={16000} rows={2} onChange={event => {
        if (readOnly) return;
        const body = event.target.value;
        retain(question.id, body ? draft?.question ?? question : null);
        setDraft(previous => body ? { body, revision: previous?.revision ?? revision, question: previous?.question ?? question } : null);
      }}/></label>
      {draft && !fresh && <div className="notice">
        <p>The question or plan changed after this answer was started. Your draft remains bound to revision {draft.revision}; no recorded answer is replaced.</p>
        {draft.question.body !== question.body && <><span>Original question</span><BoardMarkdown source={draft.question.body}/></>}
        {open && <button className="button" type="button" disabled={readOnly} onClick={() => setDraft(current => current === draft ? { ...draft, question, revision } : current)}>Keep answer for current question</button>}
      </div>}
      <div className="plan-actions">
        <button className="button" disabled={readOnly || !draft?.body.trim() || !fresh}>Answer question</button>
        {draft && <button type="button" className="button" onClick={() => setDraft(null)}>Discard answer draft</button>}
      </div>
    </form>}
  </article>;
}
