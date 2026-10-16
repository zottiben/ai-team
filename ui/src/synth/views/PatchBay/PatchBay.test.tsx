import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import PatchBay from "./PatchBay";
import { fixtureTeamOverview } from "../../team-overview-fixture";
import { contextTrace, measurements } from "../../data/telemetry";

it("renders exact evidence and hands controls back without dispatching a command", () => {
  const fixture = fixtureTeamOverview(), work = vi.fn(), review = vi.fn();
  render(<PatchBay {...fixture} onWork={work} onReview={review}/>);
  expect(screen.getByText(/Read-only evidence/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "backend · node #2" }));
  const inspector = screen.getByRole("region", { name: "Patch inspector" });
  expect(within(inspector).getByText("/fixture/draft-1")).toBeTruthy();
  expect(within(inspector).getByText(/Latest reported prompt\/context: 700/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Execution controls in Work" }));
  fireEvent.click(screen.getByRole("button", { name: "Drafts & delivery in Review" }));
  expect(work).toHaveBeenCalledOnce(); expect(review).toHaveBeenCalledOnce();
  expect(screen.queryByRole("button", { name: /Solo|Mute|Add seat|Approve/ })).toBeNull();
});
it("retains selected exact nodes through hidden presentation but resets a new owner", () => {
  const fixture = fixtureTeamOverview();
  const tree = (hidden: boolean, id = 1) => <div hidden={hidden}><PatchBay {...fixture} detail={{ ...fixture.detail, id }}/></div>;
  const view = render(tree(false)); fireEvent.click(screen.getByRole("button", { name: "frontend · node #3" }));
  view.rerender(tree(true)); view.rerender(tree(false));
  expect(screen.getByRole("button", { name: "frontend · node #3" }).getAttribute("aria-pressed")).toBe("true");
  view.rerender(tree(false, 2));
  expect(screen.queryByRole("region", { name: "Patch inspector" })).toBeNull();
  expect(screen.getByText(/No unambiguous team execution/)).toBeTruthy();
});
it("does not retarget a missing selected run or node on refresh", () => {
  const fixture = fixtureTeamOverview(); const view = render(<PatchBay {...fixture}/>);
  fireEvent.change(screen.getByLabelText("Recorded execution"), { target: { value: "1" } });
  fireEvent.click(screen.getByRole("button", { name: "backend · node #2" }));
  view.rerender(<PatchBay {...fixture} detail={{ ...fixture.detail, turns: [] }}/>);
  expect(screen.queryByRole("region", { name: "Patch inspector" })).toBeNull();
  expect(screen.getByText(/no other run was substituted/)).toBeTruthy();
  expect(screen.getByRole("option", { name: "Run #1 · unavailable" })).toBeTruthy();
});
it("keeps the two-seat reset and explicit compaction separate from estimated counts", () => {
  const { events } = fixtureTeamOverview();
  const a = contextTrace(measurements(events, 2)), b = contextTrace(measurements(events, 3));
  expect(a.values).toEqual([1000, 700]); expect(b.values).toEqual([2000, null, 1400]);
  expect(a.markers).toEqual([0]); expect(b.markers).toEqual([0]);
  expect(new Set(events.map(event => event.id)).size).toBe(events.length);
  expect(events.map(event => event.id)).toEqual(events.map(event => event.id).sort((a, b) => a - b));
});
