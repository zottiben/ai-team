import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { McpServers } from "./McpServers";
import type { McpImportCandidate, McpImportPreview, McpServer } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function server(over: Partial<McpServer> = {}): McpServer {
  return {
    id: 1,
    project_id: null,
    name: "chrome-devtools",
    enabled: false,
    transport: "stdio",
    command: "npx",
    args: ["chrome-devtools-mcp@latest"],
    url: null,
    env_keys: [],
    header_keys: [],
    include_tools: null,
    origin: "manual",
    origin_detail: "",
    rev: 3,
    updated_at: "2026-10-12T09:00:00Z",
    ...over,
  };
}

function candidate(over: Partial<McpImportCandidate> = {}): McpImportCandidate {
  return {
    name: "context7",
    source: "/home/me/.pi/agent/mcp-adapter.json",
    transport: "stdio",
    command: "context7-mcp",
    args: [],
    url: null,
    env_keys: [],
    header_keys: [],
    include_tools: null,
    importable: true,
    note: "Pinned as a copy, switched off until you enable it.",
    already: false,
    ...over,
  };
}

function preview(over: Partial<McpImportPreview> = {}): McpImportPreview {
  return {
    project_id: null,
    fingerprint: "a1b2c3",
    candidates: [],
    problems: [],
    ...over,
  };
}

type Call = { url: string; method: string; body: unknown };

/** The routes this page uses, and a record of exactly what it sent. */
function stub(
  options: {
    servers?: McpServer[];
    candidates?: McpImportCandidate[];
    preview?: McpImportPreview;
  } = {},
) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      calls.push({
        url,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      const answer = (json: unknown) =>
        Promise.resolve({ ok: true, json: async () => json } as Response);
      if (url.startsWith("/projects")) {
        return answer([
          { id: 7, slug: "widget", name: "Widget", kind: "app", status: "active", open_runs: 0 },
        ]);
      }
      if (url.startsWith("/mcp/import")) {
        if (init?.method === "POST") return answer([]);
        return answer(
          options.preview ?? preview({ candidates: options.candidates ?? [] }),
        );
      }
      if (url.startsWith("/mcp/servers")) {
        return answer(init?.method === "POST" ? server() : (options.servers ?? []));
      }
      return answer({});
    }),
  );
  return calls;
}

it("says what a configured server is and is not, before anybody enables one", async () => {
  // The two sentences that stop this page lying: AI Team does not inherit the operator's
  // own registrations, and a tick here is not a connection.
  stub({ servers: [server()] });
  render(<McpServers />);

  await screen.findByText("chrome-devtools");
  expect(screen.getByText(/do not inherit the MCP servers/i)).toBeTruthy();
  expect(screen.getByText(/Configured is not connected/i)).toBeTruthy();
  expect(screen.getByText(/may run its command on this machine/i)).toBeTruthy();
  expect(screen.getByText("off")).toBeTruthy();
  expect(screen.getByText("npx chrome-devtools-mcp@latest")).toBeTruthy();
});

it("enables a server against the revision it was showing, without resending its values", async () => {
  // The page was never given the values, so a save that carried an empty map would blank
  // a credential every time somebody ticked a box.
  const calls = stub({
    servers: [server({ enabled: false, env_keys: ["DEVTOOLS_KEY"], rev: 9 })],
  });
  render(<McpServers />);

  await userEvent.click(await screen.findByLabelText("enable chrome-devtools"));

  await waitFor(() => {
    const save = calls.find((call) => call.method === "POST");
    expect(save).toBeTruthy();
    expect(save?.url).toBe("/mcp/servers/1");
    expect(save?.body).toEqual({ rev: 9, enabled: true });
  });
});

it("shows the names of stored values and never a value", async () => {
  stub({
    servers: [
      server({
        transport: "http",
        command: null,
        args: [],
        url: "https://mcp.example.invalid",
        header_keys: ["Authorization"],
      }),
    ],
  });
  render(<McpServers />);

  await screen.findByText("chrome-devtools");
  expect(screen.getByText(/Stored headers: Authorization/)).toBeTruthy();
  expect(screen.getByText(/Values are never shown here/)).toBeTruthy();
  // Typed, not displayed: a replacement is write-only like every other secret field here.
  const field = screen.getByLabelText("chrome-devtools headers") as HTMLInputElement;
  expect(field.type).toBe("password");
  expect(field.value).toBe("");
});

it("keeps stored values when the replacement field is left blank", async () => {
  const calls = stub({ servers: [server({ env_keys: ["DEVTOOLS_KEY"], rev: 4 })] });
  render(<McpServers />);

  await screen.findByText("chrome-devtools");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));

  await waitFor(() => {
    const save = calls.find((call) => call.url === "/mcp/servers/1");
    expect(save).toBeTruthy();
    expect(save?.body).toMatchObject({ rev: 4, command: "npx" });
    expect(Object.keys(save?.body as object)).not.toContain("env");
  });
});

it("sends a replacement map when one is typed, and refuses one that is not an object", async () => {
  const calls = stub({ servers: [server({ rev: 4 })] });
  render(<McpServers />);

  await screen.findByText("chrome-devtools");
  const field = screen.getByLabelText("chrome-devtools environment variables");
  await userEvent.type(field, "not json");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(calls.some((call) => call.url === "/mcp/servers/1")).toBe(false);

  await userEvent.clear(field);
  await userEvent.type(field, '{{"DEVTOOLS_KEY": "pasted"}');
  await userEvent.click(screen.getByRole("button", { name: "Save" }));

  await waitFor(() => {
    const save = calls.find((call) => call.url === "/mcp/servers/1");
    expect(save?.body).toMatchObject({ env: { DEVTOOLS_KEY: "pasted" } });
  });
});

it("surfaces the server's refusal rather than pretending a save worked", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      if (url.startsWith("/projects")) {
        return Promise.resolve({ ok: true, json: async () => [] } as Response);
      }
      if (init?.method === "POST") {
        return Promise.resolve({
          ok: false,
          status: 400,
          json: async () => ({ error: "aip is a planner; AI Team's own planning server is generated" }),
        } as Response);
      }
      return Promise.resolve({ ok: true, json: async () => [server()] } as Response);
    }),
  );
  render(<McpServers />);

  await userEvent.click(await screen.findByLabelText("enable chrome-devtools"));
  expect(await screen.findByText(/aip is a planner/)).toBeTruthy();
});

it("asks for a project's own servers when the scope changes", async () => {
  const calls = stub({ servers: [] });
  render(<McpServers />);

  await screen.findByText(/No MCP servers configured yet/);
  await userEvent.selectOptions(screen.getByLabelText("server scope"), "7");

  await waitFor(() => {
    expect(calls.some((call) => call.url === "/mcp/servers?project=7")).toBe(true);
  });
  expect(screen.getByText(/This project's own servers/)).toBeTruthy();
});

it("imports only what is chosen, and will not offer what it cannot carry across", async () => {
  // Reviewing, then choosing. Nothing arrives because it happened to be in the file.
  const calls = stub({
    candidates: [
      candidate(),
      candidate({
        name: "signed",
        importable: false,
        note: "uses a per-request header command, which AI Team does not configure",
      }),
      candidate({ name: "file-sql", already: true }),
    ],
  });
  render(<McpServers />);

  await screen.findByText(/No MCP servers configured yet/);
  await userEvent.click(screen.getByRole("button", { name: "Look" }));

  await screen.findByText("context7");
  expect((screen.getByLabelText("import signed") as HTMLInputElement).disabled).toBe(true);
  expect(screen.getByText(/per-request header command/)).toBeTruthy();
  expect((screen.getByLabelText("import file-sql") as HTMLInputElement).disabled).toBe(true);
  expect(screen.getByText("Already configured here.")).toBeTruthy();
  expect(
    screen.getByText("Pinned as a copy, switched off until you enable it."),
  ).toBeTruthy();

  await userEvent.click(screen.getByLabelText("import context7"));
  await userEvent.click(screen.getByRole("button", { name: /^Import/ }));

  await waitFor(() => {
    const sent = calls.find((call) => call.url === "/mcp/import" && call.method === "POST");
    expect(sent?.body).toEqual({
      project_id: null,
      names: ["context7"],
      fingerprint: "a1b2c3",
    });
  });
});

it("imports against the preview it showed, not the names alone", async () => {
  // The page approved a list. Sending names back would let the server pin whatever those
  // names point at by then, which is the definition nobody reviewed.
  const calls = stub({
    preview: preview({ fingerprint: "read-at-09-00", candidates: [candidate()] }),
  });
  render(<McpServers />);

  await screen.findByText(/No MCP servers configured yet/);
  await userEvent.click(screen.getByRole("button", { name: "Look" }));
  await screen.findByText("context7");
  expect(screen.getByText(/importing this list, not these names/i)).toBeTruthy();
  expect(screen.getByText(/no command is executed and no credential is resolved/i)).toBeTruthy();
  expect(screen.getByText(/AI Team never writes to them/i)).toBeTruthy();

  await userEvent.click(screen.getByLabelText("import context7"));
  await userEvent.click(screen.getByRole("button", { name: /^Import/ }));

  await waitFor(() => {
    const sent = calls.find((call) => call.url === "/mcp/import" && call.method === "POST");
    expect((sent?.body as Record<string, unknown>).fingerprint).toBe("read-at-09-00");
  });
});

it("drops a preview when the scope changes rather than approving it for another scope", async () => {
  // A fingerprint is scope-bound, so a list read against the app defaults would be
  // refused for a project. The list goes rather than inviting a refusal.
  stub({ preview: preview({ candidates: [candidate()] }) });
  render(<McpServers />);

  await screen.findByText(/No MCP servers configured yet/);
  await userEvent.click(screen.getByRole("button", { name: "Look" }));
  await screen.findByText("context7");

  await userEvent.selectOptions(screen.getByLabelText("server scope"), "7");
  await waitFor(() => {
    expect(screen.queryByLabelText("import context7")).toBeNull();
  });
});

it("says why a Pi file was not read instead of reporting no servers", async () => {
  stub({
    preview: preview({
      problems: ["/home/me/.pi/agent/mcp.json is not JSON, so AI Team read nothing from it"],
    }),
  });
  render(<McpServers />);

  await screen.findByText(/No MCP servers configured yet/);
  await userEvent.click(screen.getByRole("button", { name: "Look" }));

  expect(await screen.findByText(/mcp\.json is not JSON/)).toBeTruthy();
});

it("warns that moving a server will not carry its stored credential across", async () => {
  // The page cannot resend values it was never given, so a change of destination needs
  // them typed again - and the help says that, rather than letting a save be refused
  // with no idea why.
  stub({ servers: [server({ env_keys: ["DEVTOOLS_KEY"] })] });
  render(<McpServers />);

  await screen.findByText("chrome-devtools");
  expect(
    screen.getByText(/will not carry these environment variables across unseen/i),
  ).toBeTruthy();
  expect(screen.getByText(/will not empty them for you/i)).toBeTruthy();
});

it("clears stored values only when an empty object is typed", async () => {
  // The one way to say "there are none now" - and it is typed, not inferred from a blank
  // field, which means keep.
  const calls = stub({ servers: [server({ env_keys: ["DEVTOOLS_KEY"], rev: 4 })] });
  render(<McpServers />);

  await screen.findByText("chrome-devtools");
  const field = screen.getByLabelText("chrome-devtools environment variables");
  await userEvent.type(field, "{{}");
  await userEvent.type(screen.getByLabelText("chrome-devtools command"), "-else");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));

  await waitFor(() => {
    const save = calls.find((call) => call.url === "/mcp/servers/1");
    expect(save?.body).toMatchObject({ rev: 4, command: "npx-else", env: {} });
  });
});

it("adds a server switched off, with its command split into arguments", async () => {
  const calls = stub({ servers: [] });
  render(<McpServers />);

  await screen.findByText(/No MCP servers configured yet/);
  await userEvent.type(screen.getByLabelText("new server name"), "chrome-devtools");
  await userEvent.type(
    screen.getByLabelText("new server location"),
    "npx chrome-devtools-mcp@latest",
  );
  await userEvent.click(screen.getByRole("button", { name: "Add" }));

  await waitFor(() => {
    const sent = calls.find((call) => call.url === "/mcp/servers" && call.method === "POST");
    expect(sent?.body).toEqual({
      project_id: null,
      name: "chrome-devtools",
      transport: "stdio",
      command: "npx",
      args: ["chrome-devtools-mcp@latest"],
    });
  });
  // Nothing in the request turns it on: that is a separate, deliberate act.
  const sent = calls.find((call) => call.url === "/mcp/servers" && call.method === "POST");
  expect((sent?.body as Record<string, unknown>).enabled).toBeUndefined();
  expect(screen.getByText(/Added switched off/)).toBeTruthy();
});

it("explains an app default where a project is selected", async () => {
  stub({ servers: [server({ project_id: null }), server({ id: 2, name: "mine", project_id: 7 })] });
  render(<McpServers />);

  await screen.findByText("chrome-devtools");
  await userEvent.selectOptions(screen.getByLabelText("server scope"), "7");

  const rows = await screen.findAllByText(/An app default|This project only/);
  expect(within(rows[0] as HTMLElement).queryByText(/./)).toBeTruthy();
  expect(screen.getByText(/Add a server of the same name here to replace it/)).toBeTruthy();
  expect(screen.getByText(/This project only/)).toBeTruthy();
});
