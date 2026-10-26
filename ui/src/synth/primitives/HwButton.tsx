import type { AnchorHTMLAttributes, ButtonHTMLAttributes } from "react";
import { Led } from "./Led";

export function HwButton({ variant = "default", latched, lockedReason, children, className = "", ...props }: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary" | "danger" | "comment" | "transport";
  latched?: boolean; lockedReason?: string;
}) {
  return <>
    <button {...props} type={props.type ?? "button"} className={`synth-key ${className}`} data-variant={variant}
      aria-pressed={latched} disabled={props.disabled || !!lockedReason} title={lockedReason ?? props.title}>
      {latched !== undefined && <Led state={latched ? "on" : "off"} size={6} />}
      {lockedReason && <span aria-hidden="true">⌑</span>}{children}
    </button>
    {lockedReason && <span className="synth-muted">{lockedReason}</span>}
  </>;
}

export function HwLink({ className = "", ...props }: AnchorHTMLAttributes<HTMLAnchorElement>) {
  return <a {...props} className={`synth-key ${className}`} />;
}
