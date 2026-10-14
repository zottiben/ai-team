import type { ButtonHTMLAttributes } from "react";
import { signalStyle, type Signal } from "./signal";

export function Jack({ label, signal = "diff", armed = false, ...props }: Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
  label: string; signal?: Signal; armed?: boolean;
}) {
  return <button {...props} type="button" className="synth-jack" aria-label={label} aria-pressed={armed} style={signalStyle(signal)}>
    <span aria-hidden="true" /><span className="synth-silk">{label}</span>
  </button>;
}
