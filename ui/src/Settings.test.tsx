import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Settings } from "./Settings";
import type { ContextSetting, Settings as SettingsData } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

/// A context source, with the fields a test is not asserting on already filled.
function source(over: Partial<ContextSetting> = {}): ContextSetting {
  return {
    source: "clickup",
    allowed: false,
    oauth_connected: false,
    token_set: false,
    held: "absent",
    token_env: "AI_TEAM_CLICKUP_TOKEN",
    ...over,
  };
}

function provider(
  over: Partial<SettingsData["providers"][number]> = {},
): SettingsData["providers"][number] {
  return {
    provider: "claude",
    label: "claude",
    allowed: false,
    reachable: false,
    detail: "denied by machine.toml",
    how: "your Claude subscription, through the Claude Code CLI",
    sign_in: "claude auth login",
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
            context: [source()],
            ...over,
          }),
        });
      }
      return Promise.resolve({
        ok: true,
        json: async () => (url === "/settings/context-auth" ? { id: 17 } : {}),
      });
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

it("a context source enabled without OAuth or a fallback says so here", async () => {
  // Rather than at the first call an agent makes, which is a long way from this page.
  stub({ context: [source({ allowed: true })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect(await screen.findByText("not connected")).toBeDefined();
});

it("an OAuth credential already held by Pi is the ready state", async () => {
  stub({ context: [source({ allowed: true, oauth_connected: true })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  expect(await screen.findByText("connected with OAuth")).toBeDefined();
  expect(screen.getByText("Reconnect")).toBeDefined();
});

it("Connect starts Pi's source-specific browser flow, not a command from the page", async () => {
  const user = userEvent.setup();
  const calls = stub({ context: [source({ allowed: true })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);

  await user.click(await screen.findByText("Connect in browser"));
  await waitFor(() =>
    expect(calls.some((call) => call.url === "/settings/context-auth")).toBe(true),
  );
  expect(calls.find((call) => call.url === "/settings/context-auth")?.body).toEqual({
    source: "clickup",
  });
});

it("the token field is blank on load, because nothing hands one back", async () => {
  // A page that showed a masked token would be a page that had fetched one, and the
  // settings route deliberately never produces a value (D23).
  stub({ context: [source({ allowed: true, token_set: true, held: "keychain" })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  await userEvent.click(await screen.findByText("Manual token fallback"));

  const field = (await screen.findByLabelText("clickup token")) as HTMLInputElement;
  expect(field.value).toBe("");
  expect(field.type, "a token typed into a visible field is a token on a screenshot").toBe(
    "password",
  );
});

it("a token is sent to its own route and the field is emptied after", async () => {
  const user = userEvent.setup();
  const calls = stub({ context: [source({ allowed: true })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  await user.click(await screen.findByText("Manual token fallback"));

  const field = (await screen.findByLabelText("clickup token")) as HTMLInputElement;
  await user.type(field, "pk_123");
  await user.click(screen.getByText("Save"));

  await waitFor(() =>
    expect(calls.some((call) => call.url === "/settings/token")).toBe(true),
  );
  expect(calls.find((call) => call.url === "/settings/token")?.body).toEqual({
    source: "clickup",
    token: "pk_123",
  });
  // Emptied, so the value is not sitting in the DOM after it has been stored.
  await waitFor(() => expect(field.value).toBe(""));
});

it("clearing sends an empty token, which is what emptying the field means", async () => {
  const user = userEvent.setup();
  const calls = stub({ context: [source({ allowed: true, token_set: true, held: "keychain" })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);
  await user.click(await screen.findByText("Manual token fallback"));

  await user.click(await screen.findByText("Clear"));

  await waitFor(() =>
    expect(calls.some((call) => call.url === "/settings/token")).toBe(true),
  );
  expect(calls.find((call) => call.url === "/settings/token")?.body).toEqual({
    source: "clickup",
    token: "",
  });
});

it("a token from the environment is shown rather than offered as editable", async () => {
  // The window cannot unset a variable its own process was started with, so a field that
  // looked like it could clear one would be a button that does nothing.
  stub({ context: [source({ allowed: true, token_set: true, held: "environment" })] });
  render(<Settings theme="dark" onTheme={() => {}} onChanged={() => {}} />);

  expect(await screen.findByText(/not editable here/)).toBeDefined();
  expect(screen.queryByLabelText("clickup token")).toBeNull();
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
