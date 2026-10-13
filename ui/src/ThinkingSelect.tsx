import { useEffect, useMemo, useState } from "react";

import {
  getModelThinking,
  isReasoning,
  REASONING_LEVELS,
  THINKING_LABELS,
  type Reasoning,
  type ThinkingCapabilities,
} from "./api";
import { SelectChevron } from "./SelectChevron";

/**
 * The thinking picker, told what the selected model can actually do.
 *
 * Three rules it exists to keep:
 *
 * - **Nothing is rewritten.** A value that arrived from a turn, a preference or a draft
 *   is shown as it is, even when the model in front of it cannot do that. Silently
 *   snapping it to the nearest supported level is how a person ends up reading a
 *   setting that was never theirs.
 * - **Unknown is not supported.** Until the lookup answers, keep the setting visible
 *   without offering unconfirmed alternatives. Sending is independent: the runtime
 *   checks its actual setting again before the first provider request.
 * - **A late answer for an abandoned model is discarded**, because enforcing one model's
 *   limits against another's is worse than enforcing none.
 */
export function ThinkingSelect({
  provider,
  model,
  value,
  disabled,
  onChange,
  title,
}: {
  /** ai-team's policy provider name. `null` while no model is chosen. */
  provider: string | null;
  model: string | null;
  /** `""` is a turn that predates thinking snapshots, not a level. */
  value: Reasoning | "";
  disabled?: boolean;
  onChange: (next: Reasoning) => void;
  title?: string;
}) {
  // `null` means "not known", which is deliberately distinct from "known to be empty".
  const [retry, setRetry] = useState(0);
  const key = useMemo(() => Symbol(), [provider, model, retry]);
  const [receipt, setReceipt] = useState<{ key: symbol | null; capabilities: ThinkingCapabilities | null; error: string | null }>({ key: null, capabilities: null, error: null });
  const capabilities = receipt.key === key ? receipt.capabilities : null;
  const supported = capabilities?.levels ?? null;
  const problem = receipt.key === key ? receipt.error : null;

  useEffect(() => {
    if (!provider || !model) return;
    // The cleanup is what makes a crossed answer harmless: the effect for the model that
    // was abandoned stops being allowed to write before its promise settles.
    let current = true;
    void getModelThinking(provider, model)
      .then((capabilities) => {
        if (!capabilities.levels.length || !capabilities.levels.every(isReasoning)) throw new Error("Invalid thinking capabilities");
        if (current) setReceipt({ key, capabilities, error: null });
      })
      .catch((error: unknown) => {
        if (current) setReceipt({ key, capabilities: null, error: error instanceof Error ? error.message : "Could not read Pi capabilities" });
      });
    return () => {
      current = false;
    };
  }, [key, provider, model]);

  const allows = (level: Reasoning) =>
    supported?.includes(level) || level === value;
  const kept = value !== "" && supported !== null && !supported.includes(value);

  return (
    <>
      <label className="chat-select chat-thinking-select" title={title}>
        <span>Thinking</span>
        <select
          aria-label="Thinking effort"
          value={value}
          disabled={disabled || supported === null}
          onChange={(event) => {
            const next = event.target.value;
            if (!disabled && isReasoning(next) && supported?.includes(next)) onChange(next);
          }}
        >
          {value === "" && <option value="">Not recorded</option>}
          {REASONING_LEVELS.map((level) => (
            <option key={level} value={level} disabled={!allows(level)}>
              {THINKING_LABELS[level]}
            </option>
          ))}
        </select>
        <SelectChevron />
      </label>
      {provider && model && <button type="button" className="button" aria-label="Refresh thinking levels" disabled={disabled || (!supported && !problem)} onClick={() => setRetry(value => value + 1)}>Refresh levels</button>}
      {capabilities?.fallback_reason && <small className="faint">{capabilities.fallback_reason}. These levels apply to {capabilities.provider}/{capabilities.model}, the policy fallback. Your requested thinking is unchanged.</small>}
      {kept && (
        <small className="faint">
          {THINKING_LABELS[value]} is not supported by this model. It is kept until you
          change it; the run refuses an unsupported level before asking the provider.
        </small>
      )}
      {problem && (
        <small className="faint">
          Supported levels are unavailable for this model. Your setting is unchanged;
          Pi will check it before model work. {problem}. Refresh levels to retry.
        </small>
      )}
    </>
  );
}
