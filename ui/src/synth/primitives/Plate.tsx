import type { HTMLAttributes, ReactNode } from "react";

export type PlateTone = "night" | "midnight" | "deck" | "magenta" | "lavender" | "ice" | "flamingo" | "yellow";
export function Plate({ tone = "midnight", radius = 18, heading, actions, children, className = "", ...props }: HTMLAttributes<HTMLElement> & {
  tone?: PlateTone; radius?: 18 | 22; heading?: ReactNode; actions?: ReactNode;
}) {
  return <section {...props} className={`synth-plate ${className}`} data-tone={tone} data-radius={radius}>
    {(heading || actions) && <header className="synth-plate__header"><span className="synth-silk">{heading}</span>{actions}</header>}
    {children}
  </section>;
}
