import { useEffect, useState } from "react";

import { models as fetchModels, type ModelChoice } from "./api";
import { createChat } from "./chat-api";
import { PLAN_STATUSES, type PlanStatus } from "./plan-api";
import { statusLabel } from "./PlanForms";
import {
  importPlan,
  planLibrary,
  previewPlanImport,
  readPlanSource,
  type PlanImportPreview,
  type PlanImportTarget,
  type PlanLibrary as Board,
  type PlanLibraryEntry,
  type PlanImported,
  type PlanSourceSurvey,
} from "./plan-library-api";
import "./plan-library.css";

/**
 * Every plan AI Team owns, across projects - and the one way a plan from the standalone
 * planner gets in here.
 *
 * The board is global because planning is: "what am I in the middle of" is rarely a
 * question about one project. Opening a plan opens the exact chat that owns it, never a
 * project's newest conversation.
 *
 * Import is deliberately four explicit steps - name a database, read it, review one
 * plan, approve it into one chat - and none of them happen on load. Nothing on this page
 * writes to the source, starts a turn, or picks a model for you.
 */
export function PlanLibrary({
  tick,
  onOpenChat,
  onChanged,
}: {
  tick: number;
  onOpenChat: (project: string, chat: number) => void;
  onChanged?: () => void;
}) {
  const [board, setBoard] = useState<Board | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [project, setProject] = useState("");
  const [status, setStatus] = useState<"" | PlanStatus>("");
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let cancelled = false;
    void planLibrary({
      project: project || null,
      status: status === "" ? [] : [status],
    })
      .then((next) => {
        if (cancelled) return;
        setBoard(next);
        setProblem(null);
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setProblem(error instanceof Error ? error.message : String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [project, status, tick, reload]);

  const refresh = () => setReload((value) => value + 1);
  return (
    <section className="plan-library" aria-label="Plans">
      <header className="plan-library-heading">
        <h2>Plans</h2>
        <p className="faint">
          Every plan AI Team owns, across projects. A plan belongs to one chat, and
          opening it goes there.
        </p>
      </header>
      {problem && (
        <div className="error" role="alert">
          {problem}{" "}
          <button className="button" onClick={refresh}>
            Try again
          </button>
        </div>
      )}
      <div className="plan-library-filters">
        <label>
          Project
          <select value={project} onChange={(event) => setProject(event.target.value)}>
            <option value="">Every project</option>
            {board?.projects.map((entry) => (
              <option key={entry.id} value={entry.slug}>
                {entry.name} ({entry.plans})
              </option>
            ))}
          </select>
        </label>
        <label>
          Status
          <select
            value={status}
            onChange={(event) => setStatus(event.target.value as "" | PlanStatus)}
          >
            <option value="">Any status</option>
            {PLAN_STATUSES.map((value) => (
              <option key={value} value={value}>
                {statusLabel(value)}
              </option>
            ))}
          </select>
        </label>
      </div>
      {!board ? (
        <p className="faint">{problem ? "The board could not be read." : "Loading plans…"}</p>
      ) : board.entries.length === 0 ? (
        <p className="faint">
          No plans here yet. Plan inside a chat, or import one from the standalone
          planner below.
        </p>
      ) : (
        <ul className="plan-library-list">
          {board.entries.map((entry) => (
            <Plan key={entry.plan_id} entry={entry} onOpenChat={onOpenChat} />
          ))}
        </ul>
      )}
      {board && board.detached.length > 0 && (
        <details className="plan-library-detached">
          <summary>Plans with no chat to open ({board.detached.length})</summary>
          <p className="faint">
            Kept, not deleted. Each one still reserves its chat id, so an import cannot
            land underneath it.
          </p>
          <ul>
            {board.detached.map((plan) => (
              <li key={plan.plan_id}>
                <strong>{plan.title}</strong> <code>{plan.slug}</code> - {plan.why}
              </li>
            ))}
          </ul>
        </details>
      )}
      {board && (
        <Import
          destinations={board.destinations}
          onImported={() => {
            refresh();
            onChanged?.();
          }}
          onOpenChat={onOpenChat}
        />
      )}
    </section>
  );
}

function Plan({
  entry,
  onOpenChat,
}: {
  entry: PlanLibraryEntry;
  onOpenChat: (project: string, chat: number) => void;
}) {
  return (
    <li className="plan-library-card">
      <div className="plan-library-card-head">
        <h3>{entry.title}</h3>
        <span className="plan-library-status" data-status={entry.status}>
          {statusLabel(entry.status)}
        </span>
      </div>
      <p className="faint">
        {entry.project_name} · {entry.chat_title}
        {entry.chat_archived && " · archived"}
      </p>
      {entry.summary && <p className="plan-library-summary">{entry.summary}</p>}
      <p className="faint">
        {entry.done} of {entry.slices} slices done
        {entry.open_questions > 0 && ` · ${entry.open_questions} open questions`}
        {entry.last_activity &&
          ` · last note ${new Date(entry.last_activity).toLocaleString()}`}
      </p>
      {entry.imported && (
        <p className="plan-library-provenance">
          Imported from <code>{entry.imported.source_path}</code>
          {entry.imported.source_plan && <> ({entry.imported.source_plan})</>} on{" "}
          {new Date(entry.imported.imported_at).toLocaleString()}. That database is not
          written to or kept in step with this copy.
        </p>
      )}
      <button className="button" onClick={() => onOpenChat(entry.project_slug, entry.chat_id)}>
        Open {entry.chat_title}
      </button>
    </li>
  );
}

/** Name a database, read it, review one plan, approve it into one chat. */
function Import({
  destinations,
  onImported,
  onOpenChat,
}: {
  destinations: PlanImportTarget[];
  onImported: () => void;
  onOpenChat: (project: string, chat: number) => void;
}) {
  const [path, setPath] = useState("");
  const [survey, setSurvey] = useState<PlanSourceSurvey | null>(null);
  const [preview, setPreview] = useState<PlanImportPreview | null>(null);
  const [chat, setChat] = useState("");
  const [done, setDone] = useState<PlanImported | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setProblem(null);
    try {
      await work();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const chosen = destinations.find((target) => String(target.chat_id) === chat);
  return (
    <details className="plan-library-import">
      <summary>Import a plan from standalone ai-planner</summary>
      <p className="faint">
        AI Team reads a private copy of the database you name and writes the plan into one
        chat you choose. It never writes to that database, registers it, or keeps the two
        in step afterwards - the imported plan is yours to edit here, and the original
        stays exactly as it is.
      </p>
      {problem && (
        <div className="error" role="alert">
          {problem}
        </div>
      )}
      <form
        className="plan-library-form"
        onSubmit={(event) => {
          event.preventDefault();
          void run(async () => {
            setPreview(null);
            setDone(null);
            setSurvey(await readPlanSource(path));
          });
        }}
      >
        <label>
          Planner database
          <input
            name="database"
            value={path}
            placeholder="/path/to/planner.db"
            onChange={(event) => setPath(event.target.value)}
            required
          />
        </label>
        <button className="button" disabled={busy}>
          Read database
        </button>
      </form>
      {survey && (
        <section className="plan-library-source" aria-label="Source database">
          <p className="faint">
            <code>{survey.source.path}</code> · {survey.source.bytes} bytes · schema{" "}
            {survey.source.schema_version} · {survey.source.plans} plans · sha256{" "}
            <code>{survey.source.digest.slice(0, 12)}</code>
          </p>
          <ul className="plan-library-source-plans">
            {survey.plans.map((plan) => (
              <li key={plan.id}>
                <div>
                  <strong>{plan.title}</strong> <code>{plan.slug}</code> ·{" "}
                  {statusLabel(plan.status)} · {plan.done}/{plan.slices} slices ·{" "}
                  {plan.repo_name}
                </div>
                {plan.already_imported ? (
                  <p className="notice">
                    Already imported into chat {plan.already_imported.chat_id} (
                    {plan.already_imported.title}) on{" "}
                    {new Date(plan.already_imported.imported_at).toLocaleString()}.
                  </p>
                ) : (
                  <button
                    className="button"
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        setDone(null);
                        setPreview(await previewPlanImport(survey.source.path, plan.id));
                      })
                    }
                  >
                    Preview {plan.slug}
                  </button>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
      {preview && (
        <section className="plan-library-preview" aria-label="Import preview">
          <h3>{preview.plan.title}</h3>
          {preview.refusal && (
            <p className="notice" role="alert">
              {preview.refusal}
            </p>
          )}
          <p className="faint">
            From <code>{preview.plan.repo_key}</code> · created{" "}
            {new Date(preview.plan.created_at).toLocaleString()} · last updated{" "}
            {new Date(preview.plan.updated_at).toLocaleString()}
          </p>
          <ul className="plan-library-counts">
            <li>{preview.counts.sections} sections</li>
            <li>{preview.counts.slices} slices</li>
            <li>{preview.counts.decisions} decisions</li>
            <li>{preview.counts.questions} questions</li>
            <li>{preview.counts.gotchas} gotchas</li>
            <li>{preview.counts.log} progress notes</li>
            <li>{preview.counts.handoffs} handoffs</li>
          </ul>
          {preview.preserved.length > 0 && (
            <>
              <h4>Carried over</h4>
              <ul>
                {preview.preserved.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </>
          )}
          <h4>Not carried over</h4>
          <ul className="plan-library-warnings">
            {preview.warnings.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
          {preview.evidence.length > 0 && (
            <>
              <h4>Recorded as history, not re-created</h4>
              <table className="plan-library-evidence">
                <thead>
                  <tr>
                    <th scope="col">Slice</th>
                    <th scope="col">Status</th>
                    <th scope="col">Claimed by</th>
                    <th scope="col">Worktree</th>
                    <th scope="col">Branch</th>
                    <th scope="col">Pull request</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.evidence.map((slice) => (
                    <tr key={slice.key}>
                      <th scope="row">
                        {slice.key} {slice.title}
                      </th>
                      <td>{statusLabel(slice.status)}</td>
                      <td>{slice.claimed_by ?? "-"}</td>
                      <td>{slice.worktree_path ?? "-"}</td>
                      <td>{slice.branch ?? "-"}</td>
                      <td>{slice.pr_url ?? "-"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <p className="faint">
                No claim, lease, branch or pull request is carried over. This import leases
                nothing, builds nothing and publishes nothing.
              </p>
            </>
          )}
          <Destination
            destinations={destinations}
            chat={chat}
            onChat={setChat}
            busy={busy}
          />
          <button
            className="button"
            disabled={busy || !chosen || preview.refusal !== null}
            onClick={() =>
              void run(async () => {
                if (!chosen) return;
                const result = await importPlan({
                  path: preview.source.path,
                  plan_id: preview.plan.id,
                  chat_id: chosen.chat_id,
                  fingerprint: preview.fingerprint,
                });
                setDone(result);
                setPreview(null);
                setSurvey(null);
                setChat("");
                onImported();
              })
            }
          >
            Import into this chat
          </button>
        </section>
      )}
      {done && (
        <section className="plan-library-done" aria-label="Imported">
          <p>
            <strong>{done.title}</strong> is now chat {done.chat_id}&rsquo;s plan in{" "}
            {done.project_slug}. {done.counts.slices} slices, {done.counts.log} progress
            notes and {done.counts.decisions} decisions came with it.
          </p>
          <button
            className="button"
            onClick={() => onOpenChat(done.project_slug, done.chat_id)}
          >
            Open the chat
          </button>
        </section>
      )}
    </details>
  );
}

/**
 * The chat the plan lands in, chosen outright.
 *
 * Only idle, empty chats with no plan of their own are offered, because those are the
 * only ones the store will accept - a picker that listed more would be offering a
 * refusal. When there is none, a chat is created here with a model the operator picks;
 * creating one never starts a turn.
 */
function Destination({
  destinations,
  chat,
  onChat,
  busy,
}: {
  destinations: PlanImportTarget[];
  chat: string;
  onChat: (value: string) => void;
  busy: boolean;
}) {
  const [creating, setCreating] = useState(false);
  return (
    <div className="plan-library-destination">
      <label>
        Import into
        <select
          value={chat}
          disabled={busy || destinations.length === 0}
          onChange={(event) => onChat(event.target.value)}
        >
          <option value="">Choose a chat</option>
          {destinations.map((target) => (
            <option key={target.chat_id} value={String(target.chat_id)}>
              {target.project_name} · {target.title} (chat {target.chat_id})
            </option>
          ))}
        </select>
      </label>
      <p className="faint">
        An imported plan becomes that chat&rsquo;s own plan, so it has to be a chat that
        is idle, has run nothing, and has no plan yet.
      </p>
      {destinations.length === 0 && !creating && (
        <button className="button" onClick={() => setCreating(true)}>
          Create an empty chat
        </button>
      )}
      {creating && <NewChat onCreated={() => setCreating(false)} />}
    </div>
  );
}

const REASONING = ["none", "low", "medium", "high"] as const;

function NewChat({ onCreated }: { onCreated: () => void }) {
  const [catalogue, setCatalogue] = useState<ModelChoice[]>([]);
  const [catalogueProblem, setCatalogueProblem] = useState<string | null>(null);
  const [project, setProject] = useState("");
  const [model, setModel] = useState("");
  const [reasoning, setReasoning] =
    useState<(typeof REASONING)[number]>("medium");
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void fetchModels()
      .then((catalog) => {
        if (cancelled) return;
        setCatalogue(catalog.models);
        setCatalogueProblem(catalog.error);
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setCatalogueProblem(
            error instanceof Error ? error.message : String(error),
          );
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <form
      className="plan-library-form"
      onSubmit={(event) => {
        event.preventDefault();
        // The option value carries both halves of the choice, because a model name
        // without the provider it belongs to is how work lands on the wrong account.
        const separator = model.indexOf("|");
        setProblem(null);
        if (separator < 0) {
          setProblem("Choose a model.");
          return;
        }
        void createChat({
          project,
          provider: model.slice(0, separator),
          model: model.slice(separator + 1),
          reasoning,
        })
          .then(() => onCreated())
          .catch((error: unknown) =>
            setProblem(error instanceof Error ? error.message : String(error)),
          );
      }}
    >
      {problem && (
        <div className="error" role="alert">
          {problem}
        </div>
      )}
      {catalogueProblem && <p className="notice">{catalogueProblem}</p>}
      <label>
        Project slug
        <input
          name="new-chat-project"
          value={project}
          onChange={(event) => setProject(event.target.value)}
          required
        />
      </label>
      <label>
        Model
        <select
          value={model}
          onChange={(event) => setModel(event.target.value)}
          required
        >
          <option value="">Choose a model</option>
          {catalogue.map((choice) => (
            <option
              key={`${choice.provider}|${choice.model}`}
              value={`${choice.provider}|${choice.model}`}
            >
              {choice.provider} · {choice.model}
            </option>
          ))}
        </select>
      </label>
      <label>
        Reasoning
        <select
          value={reasoning}
          onChange={(event) =>
            setReasoning(event.target.value as (typeof REASONING)[number])
          }
        >
          {REASONING.map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </select>
      </label>
      <button className="button">Create the chat</button>
      <p className="faint">
        A new chat waits for you. Creating one here starts no turn and sends no prompt.
      </p>
    </form>
  );
}
