import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Settings } from "./Settings";
import type { Settings as SettingsData } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function provider(over: Partial<SettingsData["providers"][number]> = {}) {
  return {
    provider: "claude",
    label: "claude",
    allowed: false,
    reachable: false,
    detail: "denied by machine.toml",
    how: "your Claude subscription, through the Claude Code CLI",
    ...over,
  };
}

function stub(over: Partial<SettingsData> = {}) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      calls.push({
        url,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      if (url === "/settings") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            profile_path: "/home/me/.config/ai-team/machine.toml",
            providers: [provider()],
            fallback: ["claude", "openai", "zai", "local"],
            context: [{ source: "clickup", allowed: false, token_set: false, token_env: "AI_TEAM_CLICKUP_TOKEN" }],
            ...over,
          }),
        });
      }
      return Promise.resolve({ ok: true, json: async () => ({}) });
    }),
  );
  return calls;
}

it("allowing a provider writes it to the machine profile", async () => {
  const user = userEvent.setup();
  const calls = stub();
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);

  await user.click(await screen.findByLabelText("allow claude"));
  await waitFor(() => expect(calls.some((c) => c.url === "/settings/provider")).toBe(true));
  expect(calls.find((c) => c.url === "/settings/provider")?.body).toMatchObject({
    provider: "claude",
    allowed: true,
  });
});

it("allowed and reachable are shown as the two different facts they are", async () => {
  // A provider ticked and not signed into fails at dispatch - minutes later, in a run,
  // somewhere else. One tick meaning both could not warn about the commonest mistake.
  stub({ providers: [provider({ allowed: true, reachable: false })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect(await screen.findByText("not signed in")).toBeDefined();

  vi.unstubAllGlobals();
  stub({ providers: [provider({ allowed: true, reachable: true })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect(await screen.findByText("ready")).toBeDefined();
});

it("a provider that is off explains how it would be authenticated", async () => {
  // Otherwise "denied" leaves somebody guessing which account it means.
  stub({ providers: [provider({ allowed: false })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect(await screen.findByText(/Claude Code CLI/)).toBeDefined();
});

it("reordering the fallback sends the whole order, not a swap", async () => {
  // The profile requires a total ranking; a partial list makes it refuse to load.
  const user = userEvent.setup();
  const calls = stub();
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);

  await user.click(await screen.findByLabelText("move openai up"));
  await waitFor(() => expect(calls.some((c) => c.url === "/settings/fallback")).toBe(true));
  expect(calls.find((c) => c.url === "/settings/fallback")?.body).toEqual({
    order: ["openai", "claude", "zai", "local"],
  });
});

it("the first in the fallback order cannot be moved up", async () => {
  stub();
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect((await screen.findByLabelText("move claude up")).getAttribute("disabled")).not.toBeNull();
});

it("a context source enabled without its token says so here", async () => {
  // Rather than at the first call an agent makes, which is a long way from this page.
  stub({
    context: [{ source: "clickup", allowed: true, token_set: false, token_env: "AI_TEAM_CLICKUP_TOKEN" }],
  });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect(await screen.findByText(/AI_TEAM_CLICKUP_TOKEN is not set/)).toBeDefined();
});

it("changing a provider tells the health indicator to re-read", async () => {
  // The readiness report changes when a provider does, and the banner is reading it.
  const user = userEvent.setup();
  stub();
  let told = 0;
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => (told += 1)} />);

  await user.click(await screen.findByLabelText("allow claude"));
  await waitFor(() => expect(told).toBeGreaterThan(0));
});

it("says where the file is, because it is meant to stay hand-editable", async () => {
  stub();
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect(await screen.findByText("/home/me/.config/ai-team/machine.toml")).toBeDefined();
});

it("switching theme is immediate and does not touch the server", async () => {
  const user = userEvent.setup();
  const calls = stub();
  const chosen: string[] = [];
  render(<Settings theme="dark" onTheme={(t) => chosen.push(t)} onChanged={() => {}} />);

  await user.click(await screen.findByText("light"));
  expect(chosen).toEqual(["light"]);
  expect(calls.every((c) => c.method === "GET")).toBe(true);
});
