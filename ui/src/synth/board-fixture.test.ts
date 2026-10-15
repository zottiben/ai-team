import { expect, it } from "vitest";
import { fixtureBoard } from "./board-fixture";

it("keeps synthetic plan status separate from slices and requires exact revision", () => {
  const fixture = fixtureBoard(), before = fixture.state().snapshot.bundle!.slices;
  expect(fixture.change({ action: "set_plan_status", status: "active", expect_revision: 4 }).status).toBe(200);
  expect(fixture.state().snapshot.bundle!.plan.status).toBe("active");
  expect(fixture.state().snapshot.bundle!.slices).toEqual(before);
  expect(fixture.change({ action: "set_plan_status", status: "done", expect_revision: 4 }).status).toBe(409);
  expect(fixture.state().snapshot.bundle!.plan.status).toBe("active");
});
it("refuses extra authority and frozen/archived synthetic status requests", () => {
  const fixture = fixtureBoard(), request = { action: "set_plan_status", status: "done", expect_revision: 4 };
  expect(fixture.change({ ...request, key: "S1" }).status).toBe(400);
  expect(fixture.change({ ...request, action: "start_run" }).status).toBe(400);
  fixture.flags({ frozen: true }); expect(fixture.change(request).status).toBe(409);
  fixture.flags({ frozen: false, archived: true }); expect(fixture.change(request).status).toBe(409);
  expect(fixture.state().snapshot.revision).toBe(4);
});
it("retains the reason requirement for a slice, not for the existing plan-status command", () => {
  const fixture = fixtureBoard();
  expect(fixture.change({ action: "set_slice_status", key: "S1", status: "blocked", expect_revision: 4 }).status).toBe(409);
  expect(fixture.change({ action: "set_plan_status", status: "blocked", expect_revision: 4 }).status).toBe(200);
});
