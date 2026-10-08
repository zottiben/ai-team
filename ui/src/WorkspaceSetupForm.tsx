import { useState } from "react";
import type { useWorkspaceSetup } from "./WorkspaceSetup";

export default function WorkspaceSetupForm({ state, disabled, onSelect }: {
  state: ReturnType<typeof useWorkspaceSetup>;
  disabled: boolean;
  onSelect: (path: string) => void;
}) {
  const [branch, setBranch] = useState("");
  const [approved, setApproved] = useState(false);
  return (
    <section className="workspace-setup">
      <div><h4>Prepare a worktree</h4><p className="faint">Set it up first, then choose the ready checkout. No agent starts.</p></div>
      <ul className="workspace-setup__summary">
        <li>AWT may reset and reuse a clean, idle pool slot, or create one.</li>
        <li>Runs configured hooks and locked dependency installs, including network access and repository scripts.</li>
        <li>If setup fails, its checkout and lease are kept for inspection.</li>
      </ul>
      <details className="chat-checkout__explanation">
        <summary>What setup runs and preserves</summary>
        <p>AWT's post-create hooks run first. AI Team refreshes locked dependencies even in warm directories: Composer before Bun/npm/pnpm/Yarn, plus recognised Rust/Python/Go setup.</p>
        <p>Your AWT hooks own environment files and project-specific setup. AI Team adds no secret copying or guessed configuration.</p>
      </details>
      {state.problem && <p className="error" role="alert">{state.problem}</p>}
      <label className="chat-checkout__field">New branch (optional)
        <input type="text" value={state.uncertain ? state.requestBranch : branch}
          disabled={state.busy || state.uncertain}
          onChange={(event) => { setBranch(event.target.value); setApproved(false); }}
          placeholder="e.g. upgrade-pretty-bytes-v7" maxLength={160} />
      </label>
      <label className="workspace-setup-consent">
        <input type="checkbox" checked={approved || state.uncertain}
          disabled={state.busy || state.uncertain}
          onChange={(event) => setApproved(event.target.checked)} />{" "}
        I approve AWT pool reuse, its hooks, network access and dependency installation.
      </label>
      <div className="workspace-setup-actions">
        <button type="button" className="button button--primary"
          disabled={disabled || !state.loaded || (!approved && !state.uncertain) || state.busy || (state.pending && !state.uncertain)}
          onClick={() => void state.start(branch)}>
          {state.uncertain ? "Retry the same setup request" : "Acquire and set up worktree"}
        </button>
        <button type="button" className="button" disabled={state.busy} onClick={state.refresh}>Refresh setup status</button>
      </div>
      {state.uncertain && <p className="notice">
        The response is uncertain. Retrying keeps the original request and branch; it cannot acquire a second checkout.
      </p>}
      {!state.loaded && <p className="faint">Loading setup receipts…</p>}
      {state.items.map((receipt) => <article className="workspace-setup-receipt" key={receipt.id}>
        <strong>Setup {receipt.id} · {receipt.state}</strong>
        <p className="mono">{receipt.branch || `ai-team/workspace-${receipt.id}`}
          {receipt.workspace_path && <> · {receipt.workspace_path}</>}</p>
        <p>{receipt.detail || "Waiting for setup to start. No agent will run yet."}</p>
        {receipt.steps.length > 0 && <details>
          <summary>Latest setup commands and output ({receipt.steps.length})</summary>
          {receipt.steps.map((step, index) => <div key={`${step.at}:${index}`}>
            <strong>{step.passed ? "Passed" : "Failed"} · <code>{step.command}</code></strong>
            <pre>{step.output}</pre>
          </div>)}
        </details>}
        {receipt.state === "ready" && receipt.workspace_path && <button type="button" className="button"
          disabled={disabled || state.pending} onClick={() => receipt.workspace_path && onSelect(receipt.workspace_path)}>
          Use ready checkout {receipt.id}
        </button>}
        {["pending", "running", "inspection"].includes(receipt.state) && <button type="button" className="button"
          disabled={state.busy} onClick={() => void state.command(receipt, "inspect")}>
          Inspect stopped setup {receipt.id}
        </button>}
        {receipt.state === "failed" && receipt.workspace_path && <button type="button" className="button"
          disabled={disabled || state.busy || state.pending} onClick={() => void state.command(receipt, "retry_dependencies")}>
          Retry dependencies in checkout {receipt.id}
        </button>}
      </article>)}
    </section>
  );
}
