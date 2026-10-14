import type { HTMLAttributes } from "react";

export function Lcd({ grid = false, variant, className = "", ...props }: HTMLAttributes<HTMLDivElement> & {
  grid?: boolean; variant?: "alert" | "working" | "comment";
}) {
  return <div {...props} className={`synth-lcd ${className}`} data-grid={grid || undefined} data-variant={variant} />;
}
