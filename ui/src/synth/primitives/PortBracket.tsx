import type { HTMLAttributes } from "react";
export function PortBracket({ className = "", ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div {...props} className={`synth-ports ${className}`} />;
}
