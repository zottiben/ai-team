import { useEffect, useState } from "react";
import { Led } from "./Led";
import { signalStyle, type Signal } from "./signal";

export interface CommandResult { id: string; exitCode: number; finishedAt: number }
export function CommandPad({ label, command, onActivate, running = false, result, disabled = false, blockedReason, signal = "edit" }: {
  label: string; command: string; onActivate: () => void; running?: boolean; result?: CommandResult;
  disabled?: boolean; blockedReason?: string; signal?: Signal;
}) {
  const [recent, setRecent] = useState(false);
  useEffect(() => {
    const remaining = result ? Math.min(2000, Math.max(0, result.finishedAt + 2000 - Date.now())) : 0;
    setRecent(remaining > 0 && !running);
    if (!remaining || running) return;
    const timer = setTimeout(() => setRecent(false), remaining);
    return () => clearTimeout(timer);
  }, [result?.id, result?.finishedAt, result?.exitCode, running]);
  const locked = disabled || running || !!blockedReason;
  return <button type="button" className="synth-command-pad" style={signalStyle(signal)} disabled={locked}
    data-hot={running || recent} title={blockedReason ?? command} onClick={() => { if (!locked) onActivate(); }}>
    <span className="synth-command-pad__heading"><span className="synth-silk">{label}</span>
      <Led state={running ? "pulse" : recent ? "on" : "off"} signal={running ? "working" : result?.exitCode === 0 ? "done" : "attention"} size={7} />
    </span>
    <code>{command}</code>
    <span className="synth-muted">{blockedReason ?? (running ? "Running" : result ? `Exit ${result.exitCode}` : "Not run")}</span>
  </button>;
}
