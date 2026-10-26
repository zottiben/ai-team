import { act, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
afterEach(() => vi.useRealTimers());
import { fixtureTeamOverview } from "../team-overview-fixture";
import { chatMeters } from "./chat-meters";
import { publishChatDisplay, releaseChatDisplay, useChatDisplay } from "./chat-display";

it("counts canonical captured input/output once, excludes caches and uses the latest measured prompt", () => {
  vi.useFakeTimers(); vi.setSystemTime(new Date("2026-10-06T09:00:14Z"));
  const fixture = fixtureTeamOverview(), now = Date.now();
  const result = chatMeters(fixture, now);
  expect(result.tokens).toBe(735); expect(result.steps).toBe(4); expect(result.context).toBe(1400); expect(result.node).toBe(3);
  expect(result.input.at(-1)).toBe(40); expect(result.output.at(-1)).toBe(9);
  expect(chatMeters({ ...fixture, events: [...fixture.events, ...fixture.events] }, now)).toEqual(result);
  const foreign = structuredClone(fixture.events.at(-1)!); foreign.id = 10000; foreign.node_run_id = 999;
  expect(chatMeters({ ...fixture, events: [...fixture.events, foreign] }, now)).toEqual(result);
});
it("never turns missing history or an unknown completed counter into zero", () => {
  const fixture = fixtureTeamOverview();
  expect(chatMeters({ ...fixture, events: [] }, Date.now()).tokens).toBeNull();
  const event = fixture.events.find(event => event.telemetry?.kind === "usage" && event.telemetry.phase === "step")!;
  if (event.telemetry?.kind === "usage") event.telemetry.tokens.input = null;
  expect(chatMeters(fixture, Date.now()).tokens).toBeNull();
});
it("scopes the ephemeral display to chat/project and an older controller cannot clear its successor", () => {
  const fixture = fixtureTeamOverview(), old = Symbol(), next = Symbol();
  const view = renderHook(({ chat, project }) => useChatDisplay(chat, project), { initialProps: { chat: 1, project: 1 } });
  act(() => publishChatDisplay(old, fixture)); expect(view.result.current).toBe(fixture);
  view.rerender({ chat: 2, project: 1 }); expect(view.result.current).toBeNull();
  view.rerender({ chat: 1, project: 2 }); expect(view.result.current).toBeNull();
  view.rerender({ chat: 1, project: 1 });
  const replacement = { detail: { ...fixture.detail, workspace_epoch: 1 }, events: [] };
  act(() => { publishChatDisplay(next, replacement); releaseChatDisplay(old); }); expect(view.result.current).toBe(replacement);
  act(() => releaseChatDisplay(next)); expect(view.result.current).toBeNull(); view.unmount();
});
