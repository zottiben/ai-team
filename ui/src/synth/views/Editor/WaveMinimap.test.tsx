import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import WaveMinimap from "./WaveMinimap";

it("keeps short-file bars, viewport and pointer jumps in the same bounded plot", () => {
  const jump = vi.fn();
  const { container } = render(<WaveMinimap lengths={Array(10).fill(20)} added={new Set()} removed={new Set()} viewport={{ from: 4, to: 6 }} onJump={jump}/>);
  const plot = container.querySelector<HTMLElement>(".synth-minimap__plot");
  expect(plot).not.toBeNull();
  expect(plot!.style.height).toBe("40px");
  expect(plot!.querySelector<HTMLElement>(".synth-minimap__window")!.style.top).toBe("30%");
  expect(plot!.querySelector<HTMLElement>(".synth-minimap__window")!.style.height).toBe("30%");
  vi.spyOn(plot!, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 100, 76, 40));
  const host = screen.getByRole("scrollbar", { name: "File map" });
  vi.spyOn(host, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 100, 76, 600));
  Object.defineProperty(host, "setPointerCapture", { value: vi.fn(), configurable: true });
  fireEvent.pointerDown(host, { clientY: 120, pointerId: 1 });
  expect(jump).toHaveBeenLastCalledWith(5);
  fireEvent.pointerMove(host, { clientY: 500, pointerId: 1 });
  expect(jump).toHaveBeenLastCalledWith(10);
});
