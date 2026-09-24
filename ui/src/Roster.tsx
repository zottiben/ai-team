import { useCallback, useEffect, useState } from "react";

import {
  detectOwnership,
  editDelivery,
  editSeat,
  models as fetchModels,
  projects as fetchProjects,
  reseatRosterModels,
  resetRosterModels,
  roster as fetchRoster,
  type DeliveryPolicy,
  type DeliverySettings,
  type ModelCatalog,
  type ModelChoice,
  type Project,
  type Reseated,
  type Roster as RosterData,
  type Seat,
} from "./api";

const DEFAULT_DELIVERY: DeliverySettings = { push: "ask", pr: "ask", merge: "ask" };

function modelValue(provider: string, model: string): string {
  return JSON.stringify([provider, model]);
}

function runtimeProvider(provider: string): string {
  return (
    {
      claude: "claude-subscription",
      openai: "openai-codex",
      zai: "zai-coding-plan",
      local: "llama.cpp",
    }[provider] ?? provider
  );
}

/**
 * Whether Pi can run a seat on this machine: what it will be dispatched as is in Pi's
 * catalogue. Core's rule too (`machine::Stranded`) - a model Pi does not list is a turn
 * that dies before a model is reached. A catalogue not read yet, or unreadable, judges
 * nothing.
 */
function runsHere(seat: Seat, catalog: ModelCatalog | null): boolean {
  if (catalog === null || catalog.error !== null) return true;
  return catalog.models.some(
    (choice) => choice.provider === seat.effective_provider && choice.model === seat.effective_model,
  );
}

function groupModels(models: ModelChoice[]): Array<[string, ModelChoice[]]> {
  const groups = new Map<string, ModelChoice[]>();
  for (const model of models) {
    const group = groups.get(model.runtime_provider) ?? [];
    group.push(model);
    groups.set(model.runtime_provider, group);
  }
  return [...groups.entries()];
}

/**
 * Who is working on what.
 *
 * The one thing this page refuses to do is show a seat as configured when dispatch will
 * resolve it differently. A seat set to a provider this machine denies falls back through
 * the ranking (D13), and a roster that hides that lies about which account the work lands
 * on - so both are shown, and the difference is called out rather than smoothed over.
 *
 * With no project selected it shows what a *new* project would get, which is the question
 * somebody asks before creating one rather than after.
 */
export function Roster({
  project = null,
  onChanged,
}: {
  project?: string | null;
  onChanged: () => void;
}) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [chosen, setChosen] = useState<string | null>(project);
  const [data, setData] = useState<RosterData | null>(null);
  const [catalog, setCatalog] = useState<ModelCatalog | null>(null);
  const [zones, setZones] = useState<Record<number, string>>({});
  const [problem, setProblem] = useState<string | null>(null);
  const [confirmDetect, setConfirmDetect] = useState(false);
  const [confirmModels, setConfirmModels] = useState(false);
  const [confirmReseat, setConfirmReseat] = useState(false);
  const [reseated, setReseated] = useState<Reseated | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [savingDelivery, setSavingDelivery] = useState(false);

  const load = useCallback(async () => {
    try {
      const [projects, roster, models] = await Promise.all([
        fetchProjects().catch(() => []),
        fetchRoster(chosen),
        fetchModels(),
      ]);
      setProjects(projects);
      setData(roster);
      setCatalog(models);
      setZones(Object.fromEntries(roster.seats.map((seat) => [seat.id, seat.zone])));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [chosen]);

  useEffect(() => {
    setChosen(project);
  }, [project]);

  useEffect(() => {
    void load();
  }, [load]);

  const change = async (id: number, what: Parameters<typeof editSeat>[1]) => {
    try {
      await editSeat(id, what);
      await load();
      onChanged();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  const changeDelivery = async (stage: keyof DeliverySettings, policy: DeliveryPolicy) => {
    if (data?.project === null || data?.project === undefined) return;
    const delivery = { ...(data.delivery ?? DEFAULT_DELIVERY), [stage]: policy };
    setSavingDelivery(true);
    setProblem(null);
    try {
      await editDelivery(data.project, delivery);
      await load();
      onChanged();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setSavingDelivery(false);
    }
  };

  const applyTeamAction = async (action: "ownership" | "models" | "reseat") => {
    if (data?.project === null || data?.project === undefined) return;
    setDetecting(true);
    try {
      let outcome: Reseated | null = null;
      if (action === "ownership") await detectOwnership(data.project);
      else if (action === "models") await resetRosterModels(data.project);
      else outcome = await reseatRosterModels(data.project);
      await load();
      // Said once the seats below show it, not a moment before over the old warnings.
      if (outcome !== null) setReseated(outcome);
      onChanged();
      setConfirmDetect(false);
      setConfirmModels(false);
      setConfirmReseat(false);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setDetecting(false);
    }
  };

  // Seats that exist and are switched on: a default roster and a seat that is off run nowhere.
  const stranded = (data?.seats ?? []).filter(
    (seat) => seat.id > 0 && seat.enabled && !runsHere(seat, catalog),
  );

  return (
    <div className="roster">
      <div className="main__header">
        <h2>Team</h2>
        {project === null ? (
          <label className="settings__toggle">
            <span className="faint">for</span>
            <select
              aria-label="project"
              value={chosen ?? ""}
              onChange={(event) => setChosen(event.target.value === "" ? null : event.target.value)}
            >
              <option value="">a new project (defaults)</option>
              {projects.map((entry) => (
                <option key={entry.id} value={entry.slug}>
                  {entry.name}
                </option>
              ))}
            </select>
          </label>
        ) : (
          <span className="faint">for this repository</span>
        )}
        {data?.team !== null && data?.team !== undefined && (
          <span className="faint mono">{data.team}</span>
        )}
        {data?.project !== null &&
          data?.project !== undefined &&
          !confirmDetect &&
          !confirmModels &&
          !confirmReseat && (
            <span className="roster__confirm">
              {stranded.length > 0 && (
                <button
                  type="button"
                  className="button button--primary"
                  onClick={() => setConfirmReseat(true)}
                >
                  Move {stranded.length} {stranded.length === 1 ? "seat" : "seats"} that cannot
                  run here
                </button>
              )}
              <button type="button" className="button" onClick={() => setConfirmDetect(true)}>
                Detect ownership
              </button>
              <button type="button" className="button" onClick={() => setConfirmModels(true)}>
                Reset role models
              </button>
            </span>
          )}
        {data?.project !== null && data?.project !== undefined && confirmDetect && (
          <span className="roster__confirm">
            <span className="faint">Replace Backend and Frontend ownership?</span>
            <button
              type="button"
              className="button button--primary"
              disabled={detecting}
              onClick={() => void applyTeamAction("ownership")}
            >
              {detecting ? "Detecting…" : "Replace ownership"}
            </button>
            <button type="button" className="button" onClick={() => setConfirmDetect(false)}>
              Cancel
            </button>
          </span>
        )}
        {data?.project !== null && data?.project !== undefined && confirmReseat && (
          <span className="roster__confirm">
            <span className="faint">
              Move {stranded.map((seat) => seat.role).join(", ")} to the models a new team here
              gets? Seats Pi can run stay as they are.
            </span>
            <button
              type="button"
              className="button button--primary"
              disabled={detecting}
              onClick={() => void applyTeamAction("reseat")}
            >
              {detecting ? "Reading Pi…" : "Move them"}
            </button>
            <button type="button" className="button" onClick={() => setConfirmReseat(false)}>
              Cancel
            </button>
          </span>
        )}
        {data?.project !== null && data?.project !== undefined && confirmModels && (
          <span className="roster__confirm">
            <span className="faint">Replace every seat's model with its role default?</span>
            <button
              type="button"
              className="button button--primary"
              disabled={detecting}
              onClick={() => void applyTeamAction("models")}
            >
              {detecting ? "Reading Pi…" : "Replace models"}
            </button>
            <button type="button" className="button" onClick={() => setConfirmModels(false)}>
              Cancel
            </button>
          </span>
        )}
      </div>

      {problem !== null && <p className="error">{problem}</p>}
      {reseated !== null && (
        <div className="notice roster__reseated" role="status">
          <span>
            {reseated.moved.length === 0
              ? "Nothing could be moved."
              : `Moved ${reseated.moved.length} ${reseated.moved.length === 1 ? "seat" : "seats"} to models this machine can run.`}
          </span>
          <ul>
            {reseated.moved.map((seat) => (
              <li key={seat.role}>
                {seat.role} → <span className="mono">{seat.to}</span>
              </li>
            ))}
            {reseated.left.map((seat) => (
              <li key={seat.role}>
                {seat.role} stays: {seat.why}
              </li>
            ))}
          </ul>
        </div>
      )}
      {data === null && <p className="empty">Reading the roster…</p>}

      {data !== null && data.project === null && (
        <p className="faint">
          These are the seats a new project gets, with an exact available model chosen for
          each role. Change them per project once it exists.
        </p>
      )}

      {data?.project !== null && data?.project !== undefined && (
        <section className="roster__delivery" aria-label="Delivery approvals">
          <div>
            <strong>Delivery approvals</strong>
            <p className="faint">
              Commit is automatic. Choose who may cross each remote boundary after verification.
            </p>
          </div>
          {(["push", "pr", "merge"] as const).map((stage) => (
            <label key={stage}>
              <span>{stage === "push" ? "Push" : stage === "pr" ? "Open PR" : "Merge"}</span>
              <select
                aria-label={`${stage} policy`}
                disabled={savingDelivery}
                value={(data.delivery ?? DEFAULT_DELIVERY)[stage]}
                onChange={(event) =>
                  void changeDelivery(stage, event.target.value as DeliveryPolicy)
                }
              >
                <option value="manual">Manual</option>
                <option value="ask">Ask me</option>
                <option value="auto">Automatic</option>
              </select>
            </label>
          ))}
        </section>
      )}

      {catalog?.error !== null && catalog?.error !== undefined && (
        <p className="error">Could not read Pi's model catalogue: {catalog.error}</p>
      )}

      {catalog !== null && catalog.error === null && catalog.models.length === 0 && (
        <p className="error">
          Pi reports no usable model on an allowed provider. Sign in or allow one in Settings.
        </p>
      )}

      <div className="list">
        {data?.seats.map((seat) => {
          const differs =
            seat.effective_provider !== seat.provider || seat.effective_model !== seat.model;
          return (
            <div key={`${seat.role}-${seat.id}`} className="card">
              <div className="card__row">
                <span className="roster__seat">
                  <span>{seat.name}</span>
                  <span className="faint mono">{seat.role}</span>
                </span>
                <span className="roster__seat">
                  <span className="status" data-status={seat.read_only ? "queued" : "running"}>
                    {seat.read_only ? "reads" : "writes"}
                  </span>
                  {!seat.enabled && (
                    <span className="status" data-status="cancelled">
                      off
                    </span>
                  )}
                </span>
              </div>
              <span className="faint">{seat.purpose}</span>

              <div className="card__row">
                <label className="roster__model">
                  <span className="faint">model</span>
                  <select
                    aria-label={`model for ${seat.role}`}
                    value={modelValue(seat.provider, seat.model)}
                    disabled={seat.id < 0 || catalog === null}
                    onChange={(event) => {
                      const [provider, model] = JSON.parse(event.target.value) as [string, string];
                      void change(seat.id, { provider, model });
                    }}
                  >
                    {!catalog?.models.some(
                      (choice) => choice.provider === seat.provider && choice.model === seat.model,
                    ) && (
                      <option value={modelValue(seat.provider, seat.model)}>
                        {runtimeProvider(seat.provider)}/{seat.model} (unavailable)
                      </option>
                    )}
                    {groupModels(catalog?.models ?? []).map(([provider, choices]) => (
                      <optgroup key={provider} label={provider}>
                        {choices.map((choice) => (
                          <option
                            key={`${choice.provider}/${choice.model}`}
                            value={modelValue(choice.provider, choice.model)}
                          >
                            {choice.model} · {choice.context} context · {choice.max_output} max
                          </option>
                        ))}
                      </optgroup>
                    ))}
                  </select>
                </label>
              </div>

              {seat.id > 0 && seat.enabled && !runsHere(seat, catalog) && (
                <span className="error">
                  Cannot run on this machine: Pi has no{" "}
                  {runtimeProvider(seat.effective_provider)}/{seat.effective_model}, so its turns
                  fail before a model is reached.
                </span>
              )}

              {/* The whole point: what it will actually be, when that is not what it says. */}
              {differs && (
                <span className="notice">
                  Runs as {seat.effective_provider}/{seat.effective_model}
                  {seat.fallback_reason === null ? "" : ` - ${seat.fallback_reason}`}
                </span>
              )}

              {seat.id > 0 && !seat.read_only ? (
                <div className="roster__ownership">
                  <label>
                    <span className="faint">owns · one glob per line</span>
                    <textarea
                      className="mono"
                      aria-label={`ownership for ${seat.role}`}
                      rows={Math.max(3, Math.min(4, (zones[seat.id] ?? seat.zone).split("\n").length))}
                      value={zones[seat.id] ?? seat.zone}
                      onChange={(event) =>
                        setZones((current) => ({ ...current, [seat.id]: event.target.value }))
                      }
                    />
                  </label>
                  {(zones[seat.id] ?? seat.zone).trim() === "" && (
                    <span className="faint">nothing yet</span>
                  )}
                  <button
                    type="button"
                    className="button"
                    disabled={(zones[seat.id] ?? seat.zone) === seat.zone}
                    onClick={() => void change(seat.id, { zone: zones[seat.id] ?? "" })}
                  >
                    Save ownership
                  </button>
                </div>
              ) : (
                <div className="card__row">
                  <span className="faint">owns</span>
                  <span className="mono">
                    {seat.zone.trim() === ""
                      ? seat.read_only
                        ? "nothing - it does not edit"
                        : "nothing yet"
                      : seat.zone.split("\n").join(", ")}
                  </span>
                </div>
              )}

              {seat.id > 0 && (
                <div className="card__row">
                  <button
                    type="button"
                    className="button"
                    onClick={() => void change(seat.id, { enabled: !seat.enabled })}
                  >
                    {seat.enabled ? "Disable" : "Enable"}
                  </button>
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Said once, because an edit that looks applied and is not is the confusing case. */}
      <p className="faint">
        Changes take effect at each seat's next turn, including in a run already going.
      </p>
    </div>
  );
}
