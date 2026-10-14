import { REASONING_LEVELS, THINKING_LABELS, type Reasoning } from "../../../api";
import { Knob } from "../../primitives/Knob";
import "../../tokens.css";
import "../../primitives.css";
import "./composer.css";

/** Presentation only: ThinkingSelect still owns the one model-scoped capability receipt. */
export default function ThinkingKnob({ value, supported, disabled, onChange, title }: {
  value: Reasoning | ""; supported: Reasoning[] | null; disabled?: boolean;
  onChange: (next: Reasoning) => void; title?: string;
}) {
  const levels: (Reasoning | "")[] = REASONING_LEVELS.filter(level => supported?.includes(level) || level === value);
  if (value === "") levels.unshift("");
  const labels = levels.map(level => level === "" ? "Not recorded" : THINKING_LABELS[level]);
  const position = Math.max(0, levels.indexOf(value)) / Math.max(1, levels.length - 1);
  return <div className="synth-thinking-control" title={title}>
    <Knob label="Thinking effort" value={position} defaultValue={position} detents={labels} signal="think" size={52}
      disabled={disabled || supported === null} onCommit={next => {
        const level = levels[Math.round(next * Math.max(0, levels.length - 1))];
        if (!disabled && level && level !== value && supported?.includes(level)) onChange(level);
      }}/>
  </div>;
}
