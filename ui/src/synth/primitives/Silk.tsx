import type { HTMLAttributes } from "react";
export function Silk({ className = "", ...props }: HTMLAttributes<HTMLSpanElement>) {
  return <span {...props} className={`synth-silk ${className}`} />;
}
