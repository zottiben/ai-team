import { useEffect, useRef, useState } from "react";
import {
  commandWorkspaceSetup,
  startWorkspaceSetup,
  workspaceSetups,
  type WorkspaceSetupReceipt,
} from "./chat-workspace-api";

/** Mounted with the composer, not the popover: closing a menu cannot lose a pending
 * request's identity or silently unblock Send during checkout setup. */
export function useWorkspaceSetup(
  project: string,
  enabled: boolean,
  tick: number,
) {
  const [items, setItems] = useState<WorkspaceSetupReceipt[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const working = useRef(false);
  const serial = useRef(0);
  const request = useRef<{ id: string; branch: string } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    const generation = ++serial.current;
    let current = true;
    void workspaceSetups(project)
      .then((found) => {
        if (!current || serial.current !== generation) return;
        setItems((previous) =>
          found.map((item) => {
            const newer = previous.find(
              (old) => old.id === item.id && old.rev > item.rev,
            );
            return newer ?? item;
          }),
        );
        setLoaded(true);
        if (
          request.current &&
          found.some(
            (item) =>
              item.request_id === request.current?.id &&
              item.branch === request.current.branch,
          )
        ) {
          request.current = null;
          setUncertain(false);
          setProblem(null);
        }
      })
      .catch((error: unknown) => {
        if (current && serial.current === generation)
          setProblem(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, [project, enabled, tick, refresh]);
  const act = async (work: () => Promise<WorkspaceSetupReceipt>) => {
    if (working.current) return;
    working.current = true;
    ++serial.current;
    setBusy(true);
    setProblem(null);
    try {
      const receipt = await work();
      setItems((previous) => [
        receipt,
        ...previous.filter((item) => item.id !== receipt.id),
      ]);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      working.current = false;
      setBusy(false);
      setRefresh((value) => value + 1);
    }
  };
  const pending =
    enabled &&
    (busy ||
      uncertain ||
      items.some((item) =>
        ["pending", "running", "inspection"].includes(item.state),
      ));
  return {
    items,
    loaded,
    busy,
    uncertain,
    pending,
    problem,
    requestBranch: request.current?.branch ?? "",
    refresh: () => setRefresh((value) => value + 1),
    start: (branch: string) =>
      act(async () => {
        request.current ??= { id: crypto.randomUUID(), branch: branch.trim() };
        setUncertain(true);
        const receipt = await startWorkspaceSetup(
          project,
          request.current.id,
          request.current.branch,
        );
        request.current = null;
        setUncertain(false);
        return receipt;
      }),
    command: (
      receipt: WorkspaceSetupReceipt,
      action: "inspect" | "retry_dependencies",
    ) => act(() => commandWorkspaceSetup(project, receipt, action)),
  };
}

export function WorkspaceSetup({
  state,
  disabled,
  onSelect,
}: {
  state: ReturnType<typeof useWorkspaceSetup>;
  disabled: boolean;
  onSelect: (path: string) => void;
}) {
  const [branch, setBranch] = useState("");
  const [approved, setApproved] = useState(false);
  return (
    <details className="workspace-setup">
      <summary>Set up a new AWT worktree</summary>
      <p className="faint">
        AWT reserves a pooled checkout. It may reset and reuse a clean idle
        slot, or create one. Its configured post-create hooks run first; AI Team
        then installs locked dependencies (Composer before Bun/npm/pnpm/Yarn,
        plus recognised Rust/Python/Go setup). Warm dependency directories are
        refreshed too.
      </p>
      <p className="faint">
        This can use the network and run repository install scripts. Your AWT
        hooks remain responsible for environment files and project-specific
        setup; AI Team adds no secret copying or guessed configuration. No agent
        starts. Failed setup keeps its checkout and lease for inspection.
      </p>
      {state.problem && (
        <p className="error" role="alert">
          {state.problem}
        </p>
      )}
      <label>
        New branch (optional)
        <input
          value={state.uncertain ? state.requestBranch : branch}
          disabled={state.busy || state.uncertain}
          onChange={(event) => {
            setBranch(event.target.value);
            setApproved(false);
          }}
          placeholder="Generated branch name if blank"
          maxLength={160}
        />
      </label>
      <label className="workspace-setup-consent">
        <input
          type="checkbox"
          checked={approved || state.uncertain}
          disabled={state.busy || state.uncertain}
          onChange={(event) => setApproved(event.target.checked)}
        />{" "}
        I approve AWT pool reuse, its hooks, network access and dependency
        installation.
      </label>
      <div className="workspace-setup-actions">
        <button
          type="button"
          className="button"
          disabled={
            disabled ||
            !state.loaded ||
            (!approved && !state.uncertain) ||
            state.busy ||
            (state.pending && !state.uncertain)
          }
          onClick={() => void state.start(branch)}
        >
          {state.uncertain
            ? "Retry the same setup request"
            : "Acquire and set up worktree"}
        </button>
        <button
          type="button"
          className="button"
          disabled={state.busy}
          onClick={state.refresh}
        >
          Refresh setup status
        </button>
      </div>
      {state.uncertain && (
        <p className="notice">
          The response is uncertain. Retrying keeps the original request and
          branch; it cannot acquire a second checkout.
        </p>
      )}
      {!state.loaded && <p className="faint">Loading setup receipts…</p>}
      {state.items.map((receipt) => (
        <article className="workspace-setup-receipt" key={receipt.id}>
          <strong>
            Setup {receipt.id} · {receipt.state}
          </strong>
          <p className="mono">
            {receipt.branch || `ai-team/workspace-${receipt.id}`}
            {receipt.workspace_path && <> · {receipt.workspace_path}</>}
          </p>
          <p>
            {receipt.detail ||
              "Waiting for setup to start. No agent will run yet."}
          </p>
          {receipt.steps.length > 0 && (
            <details>
              <summary>
                Latest setup commands and output ({receipt.steps.length})
              </summary>
              {receipt.steps.map((step, index) => (
                <div key={`${step.at}:${index}`}>
                  <strong>
                    {step.passed ? "Passed" : "Failed"} ·{" "}
                    <code>{step.command}</code>
                  </strong>
                  <pre>{step.output}</pre>
                </div>
              ))}
            </details>
          )}
          {receipt.state === "ready" && receipt.workspace_path && (
            <button
              type="button"
              className="button"
              disabled={disabled || state.pending}
              onClick={() =>
                receipt.workspace_path && onSelect(receipt.workspace_path)
              }
            >
              Use ready checkout {receipt.id}
            </button>
          )}
          {["pending", "running", "inspection"].includes(receipt.state) && (
            <button
              type="button"
              className="button"
              disabled={state.busy}
              onClick={() => void state.command(receipt, "inspect")}
            >
              Inspect stopped setup {receipt.id}
            </button>
          )}
          {receipt.state === "failed" && receipt.workspace_path && (
            <button
              type="button"
              className="button"
              disabled={disabled || state.busy || state.pending}
              onClick={() => void state.command(receipt, "retry_dependencies")}
            >
              Retry dependencies in checkout {receipt.id}
            </button>
          )}
        </article>
      ))}
    </details>
  );
}
