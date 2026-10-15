import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import PlanHeader, { PlanTabs } from "./PlanHeader";
import { PlanKanban } from "../../../PlanKanban";
import type { PlanSlice, PlanStatus } from "../../../plan-api";
import { setSynth } from "../../../ui-preferences";

const slice = (key: string, status: PlanStatus): PlanSlice => ({ id: Number(key.slice(1)), key, title: `Slice ${key}`, status, scope_md: "Scope", demo_md: null, blocked_reason: null, rev: 1 });
beforeEach(() => setSynth(false));
it("keeps the authoritative pattern order and separates focus from reveal", () => {
  const reveal = vi.fn();
  render(<PlanHeader slices={[slice("S7", "draft"), slice("S1", "active"), slice("S9", "done")]} onReveal={reveal}/>);
  const buttons = screen.getAllByRole("button", { name: /^Reveal/ });
  expect(buttons.map(button => button.textContent)).toEqual(["S7", "S1", "S9"]);
  expect(screen.getByLabelText("Reported slice count").textContent).toBe("1/3");
  act(() => buttons[0]!.focus()); fireEvent.keyDown(buttons[0]!, { key: "ArrowRight" });
  expect(document.activeElement).toBe(buttons[1]); expect(reveal).not.toHaveBeenCalled();
  fireEvent.keyDown(buttons[1]!, { key: "End" }); expect(document.activeElement).toBe(buttons[2]);
  fireEvent.click(buttons[2]!); expect(reveal).toHaveBeenCalledWith("S9");
});
it("has chat-instance-scoped tab controls and manual keyboard activation", () => {
  const change = vi.fn();
  render(<><PlanTabs prefix="first" view="board" questions={2} onChange={change}/><PlanTabs prefix="second" view="questions" questions={0} onChange={change}/></>);
  const first = within(screen.getAllByRole("tablist")[0]!);
  const board = first.getByRole("tab", { name: "Board" }), questions = first.getByRole("tab", { name: /Open questions/ });
  expect(questions.getAttribute("aria-controls")).toBe("first-panel-questions");
  expect(first.getByText("2")).toBeTruthy();
  board.focus(); fireEvent.keyDown(board, { key: "ArrowRight" });
  expect(document.activeElement).toBe(first.getByRole("tab", { name: "Plan" })); expect(change).not.toHaveBeenCalled();
  fireEvent.click(questions); expect(change).toHaveBeenCalledWith("questions");
  const ids = [...document.querySelectorAll("[id]")].map(element => element.id);
  expect(new Set(ids).size).toBe(ids.length);
});
it("caps meter illumination at eight without turning the count into a WIP limit", () => {
  setSynth(true);
  render(<PlanKanban slices={Array.from({ length: 12 }, (_, index) => slice(`S${index + 1}`, "ready"))} revision={4} disabled save={async () => false}/>);
  const lane = screen.getByRole("region", { name: "Plan board" }).querySelector('[data-status="ready"]')!;
  expect(lane.textContent).toContain("Slices · capped meter, not a limit · 12");
  expect(lane.querySelectorAll('[data-lit="true"]')).toHaveLength(8);
  expect(lane.querySelectorAll(".plan-slice-card")).toHaveLength(12);
  act(() => setSynth(false));
});
