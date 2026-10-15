import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { RunEvent } from "../../../api";
import TerminalWorkspace from "./TerminalWorkspace";
import { TerminalPane } from "../../../Terminal";
import { setSynth } from "../../../ui-preferences";
import { CommandPads } from "./CommandPads";
import { SynthProvider } from "../../SynthProvider";

/**
 * The terminal workspace under the skin it was designed for.
 *
 * xterm needs a canvas, so it is a fixture here - which is also what lets these tests
 * drive the parts that matter: what reaches the server, and what cannot.
 */
const xterm = vi.hoisted(() => ({
  input: null as ((text: string) => void) | null,
  selectionChanged: () => {},
  selection: "",
  written: [] as string[],
  options: {} as Record<string, unknown>,
  instances: 0,
  webgl: "ok" as "ok" | "throws",
  contextLoss: null as (() => void) | null,
  webglDisposed: 0,
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    rows = 24;
    cols = 80;
    buffer = { active: { viewportY: 0 } };
    options: Record<string, unknown>;
    constructor(options: Record<string, unknown>) {
      this.options = options;
      xterm.options = options;
      xterm.instances += 1;
    }
    loadAddon() {}
    open() {}
    write(text: string) {
      xterm.written.push(text);
    }
    dispose() {}
    onData(callback: (text: string) => void) {
      xterm.input = callback;
    }
    onSelectionChange(callback: () => void) {
      xterm.selectionChanged = callback;
      return { dispose() {} };
    }
    getSelection() {
      return xterm.selection;
    }
    getSelectionPosition() {
      return xterm.selection ? { start: { x: 2, y: 3 }, end: { x: 9, y: 3 } } : undefined;
    }
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
// A machine without a GL2 context throws here, not on import - and the import itself
// can fail too. Both land in the same fallback.
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    constructor() {
      if (xterm.webgl === "throws") throw new Error("WebGL2 is not available");
    }
    onContextLoss(callback: () => void) {
      xterm.contextLoss = callback;
    }
    dispose() {
      xterm.webglDisposed += 1;
    }
  },
}));

it("retains the xterm instance and input owner across skins", async () => {
  act(() => setSynth(true));
  render(<TerminalPane project="p" node={null} />);
  await waitFor(() => expect(xterm.instances).toBe(1));
  const input = xterm.input;
  act(() => setSynth(false));
  await waitFor(() => expect(screen.getByRole("button", { name: "Restart" })).toBeTruthy());
  expect(xterm.instances).toBe(1);
  expect(xterm.input).toBe(input);
  act(() => setSynth(true));
  await waitFor(() => expect(screen.getByRole("tab", { name: /agent/i })).toBeTruthy());
  expect(xterm.instances).toBe(1);
});

type Call = { url: string; method: string; body: unknown };
const server = vi.hoisted(() => ({
  sessions: [] as unknown[],
  pads: [] as unknown[],
  deck: { worktree: "/w", running: [] as unknown[], history: [] as unknown[] },
  chunks: [] as unknown[],
  calls: [] as Call[],
  runFails: null as string | null,
  /** What the press answers with. A replay is the server saying "already done". */
  runReplay: null as Record<string, unknown> | null,
  deckFails: false,
  writeFails: false,
}));

/** A started receipt, as the server answers a fresh press. */
const receipt = {
  id: 1,
  request_id: "r1",
  token: "t1",
  pad_id: 3,
  revision: 1,
  label: "Check",
  command: "cargo test",
  worktree: "/w",
  dispatch: "started",
  refusal: null,
  session_id: 77,
  claimed_at: "now",
  started_at: "now",
  finished_at: null,
  exit_code: null,
  duration_ms: null,
  lines: null,
  lines_exact: true,
  state: "running",
  replayed: false,
};

it("keeps a pad confirmation bound to the checkout that opened it", () => {
  const onRun = vi.fn();
  const props = { pads: [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" }],
    deck: null, worktree: "/old", disabled: null, problem: null, onRun, onAdd: vi.fn(), onEdit: vi.fn(), onRemove: vi.fn() };
  const view = render(<CommandPads {...props} />, { wrapper: SynthProvider });
  fireEvent.click(screen.getByRole("button", { name: /^Check/ }));
  view.rerender(<CommandPads {...props} worktree="/new" />);
  fireEvent.click(screen.getByRole("button", { name: "Run it" }));
  expect(onRun).not.toHaveBeenCalled();
});

it("does not lend a receipt to another pad with the same command text", () => {
  const pads = [{ id: 3, position: 0, revision: 1, label: "First", command: "cargo test" },
    { id: 4, position: 1, revision: 1, label: "Second", command: "cargo test" }];
  const deck = { worktree: "/w", running: [], history: [{ ...receipt, state: "finished" as const,
    dispatch: "started" as const, finished_at: "2026-10-15T01:00:00Z", exit_code: 0 }] };
  render(<CommandPads pads={pads} deck={deck} worktree="/w" disabled={null} problem={null}
    onRun={vi.fn()} onAdd={vi.fn()} onEdit={vi.fn()} onRemove={vi.fn()} />, { wrapper: SynthProvider });
  expect(screen.getByRole("button", { name: /^First/ }).textContent).toContain("Exit 0");
  expect(screen.getByRole("button", { name: /^Second/ }).textContent).toContain("Not run");
});

function stub() {
  server.calls = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      server.calls.push({
        url,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      const ok = (value: unknown) => Promise.resolve({ ok: true, json: async () => value });
      const bad = (error: string) => Promise.resolve({ ok: false, status: 400, json: async () => ({ error }) });
      if (url.startsWith("/terminals/pads")) return ok(init?.method === "POST" ? { id: 9, position: 0, revision: 1, label: "New", command: "ls" } : server.pads);
      if (url.startsWith("/terminals/commands")) {
        if (init?.method === "POST")
          return server.runFails ? bad(server.runFails) : ok({ ...receipt, ...server.runReplay });
        return server.deckFails ? bad("no checkout") : ok(server.deck);
      }
      if (url.startsWith("/terminals?")) return ok(server.sessions);
      if (url === "/terminals") return ok({ id: 42 });
      if (/^\/terminals\/\d+$/.test(url)) return server.writeFails ? bad("Connection lost") : ok({ sent: 1 });
      if (/^\/terminals\/\d+\?/.test(url)) return ok(server.chunks.shift() ?? { text: "", cursor: 0, done: false, status: null, dropped: 0 });
      return ok({});
    }),
  );
}

const shell = { id: 7, worktree: "/w", done: false, status: null, kind: "shell", label: "Shell", duration_ms: 1_000 };

beforeEach(() => {
  server.sessions = [shell];
  server.pads = [];
  server.deck = { worktree: "/w", running: [], history: [] };
  server.chunks = [];
  server.runFails = null;
  server.runReplay = null;
  server.deckFails = false;
  server.writeFails = false;
  xterm.input = null;
  xterm.selection = "";
  xterm.written = [];
  xterm.instances = 0;
  xterm.webgl = "ok";
  xterm.contextLoss = null;
  xterm.webglDisposed = 0;
  stub();
});

afterEach(() => {
  setSynth(false);
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const posted = (path: string) => server.calls.filter((call) => call.method === "POST" && call.url === path);
const postedUnder = (path: string) => server.calls.filter((call) => call.method === "POST" && call.url.startsWith(path));
/** Keystrokes. The one request the mirror must never be able to make. */
const writes = () => server.calls.filter((call) => call.method === "POST" && /^\/terminals\/\d+$/.test(call.url));

function bash(id: number, kind: string, tool: Record<string, unknown>): RunEvent {
  return {
    id,
    node_run_id: 1,
    session_id: "fixture-session",
    kind,
    actor: "agent",
    summary: "bash",
    message: null,
    thinking: [],
    at: "2026-10-15T09:00:00Z",
    tool: { call_id: "c1", parent_call_id: null, name: "bash", target: null, outcome: null, duration_ms: null, output: null, ...tool } as RunEvent["tool"],
  };
}

it("rejoins the sessions this checkout already has rather than opening another", async () => {
  // Closing a window must not orphan the four-minute build, and opening a second shell
  // beside the live one is the same mistake with extra steps.
  server.sessions = [shell, { ...shell, id: 8 }];
  render(<TerminalWorkspace project="widget" node={null} />);

  await screen.findByRole("tab", { name: /shell 1/ });
  expect(screen.getByRole("tab", { name: /shell 2/ })).toBeDefined();
  expect(posted("/terminals")).toHaveLength(0);
});

it("opens one shell for a checkout that has none, and only one", async () => {
  // Opening the terminal is asking for a shell. Asking twice is not - a window that
  // reopened one on every refresh would leave a trail of login shells behind it.
  server.sessions = [];
  const view = render(<TerminalWorkspace project="widget" node={null} />);
  await waitFor(() => expect(posted("/terminals")).toHaveLength(1));

  view.rerender(<TerminalWorkspace project="widget" node={null} workspace={null} />);
  await waitFor(() => expect(screen.getByRole("tab", { name: /agent/ })).toBeDefined());
  expect(posted("/terminals")).toHaveLength(1);
});

it("retains unanswered request identities while a different pad is pressed", async () => {
  server.pads = [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" },
    { id: 4, position: 1, revision: 1, label: "Other", command: "cargo fmt" }];
  server.runFails = "Lost answer";
  render(<TerminalWorkspace project="widget" node={null} />);
  await run();
  server.runFails = null;
  await run(/^Other/);
  await run();
  expect(pressBody(2).request_id).toBe(pressBody(0).request_id);
  expect(pressBody(1).request_id).not.toBe(pressBody(0).request_id);
});

it("gives the agent's shell a tab with no session behind it", async () => {
  const onSend = vi.fn();
  render(
    <TerminalWorkspace
      project="widget"
      node={null}
      onSend={onSend}
      events={[
        bash(1, "tool_call", { target: { kind: "command", text: "cargo build", truncated: false } }),
        bash(2, "note", { partial: true, output: { text: "Compiling ai-team-core", truncated: true } }),
      ]}
    />,
  );

  const tab = await screen.findByRole("tab", { name: /agent · bash \(read-only mirror\)/ });
  await act(async () => {
    fireEvent.click(tab);
  });
  expect(screen.getAllByText("cargo build").length).toBeGreaterThan(0);
  expect(screen.getByText(/sampled tail, may omit output/)).toBeDefined();
  expect(screen.getByText(/output truncated/)).toBeDefined();
  expect(screen.getByText(/there is no input to it from here/)).toBeDefined();

  // The operator's terminal stays mounted but hidden. The mirror has no input owner.
  expect(screen.getByLabelText("Shell").closest("[hidden]")).toBeTruthy();
  expect(screen.getByLabelText("Agent shell, read only").querySelector("input,textarea:not([readonly]),[contenteditable=true]")).toBeNull();
  expect(screen.getByLabelText("Agent shell, read only")).toBeDefined();
  expect(writes()).toHaveLength(0);
  const output = screen.getByLabelText("Recorded shell output 1") as HTMLTextAreaElement;
  output.setSelectionRange(0, 9);
  fireEvent.select(output);
  expect(onSend).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Send to agent" }));
  expect(onSend).toHaveBeenCalledWith("Compiling", "agent_shell");
  expect(writes()).toHaveLength(0);
  fireEvent.click(screen.getByRole("tab", { name: /shell 1/i }));
  expect(xterm.instances).toBe(1);
});

/** What the nth press carried. */
function pressBody(index: number): Record<string, unknown> {
  const call = posted("/terminals/commands")[index];
  if (!call) throw new Error(`no press at ${index}`);
  return call.body as Record<string, unknown>;
}

/** Open a pad's confirmation and press Run it. */
async function run(name = /^Check/) {
  const pad = await screen.findByRole("button", { name });
  await act(async () => {
    fireEvent.click(pad);
  });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Run it" }));
  });
}

it("asks before running a pad, names the exact command and checkout, and types nowhere", async () => {
  server.pads = [{ id: 3, position: 0, revision: 4, label: "Check", command: "cargo test" }];
  render(<TerminalWorkspace project="widget" node={null} />);

  const pad = await screen.findByRole("button", { name: /^Check/ });
  await act(async () => {
    fireEvent.click(pad);
  });
  const confirm = screen.getByRole("dialog", { name: "Run this command" });
  expect(confirm.textContent).toContain("cargo test");
  expect(confirm.textContent).toContain("/w");
  expect(confirm.textContent).toContain("Your shells, and anything typed in them, are left alone");
  expect(posted("/terminals/commands")).toHaveLength(0);

  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Run it" }));
  });
  await waitFor(() => expect(posted("/terminals/commands")).toHaveLength(1));
  const body = pressBody(0);
  expect(body).toMatchObject({
    project: "widget",
    workspace: null,
    node: null,
    pad: 3,
    // The press carries everything it was confirmed against: the pad as it was read, and
    // the checkout the dialog named rather than whichever one the deck is on now.
    revision: 4,
    confirmed: "cargo test",
    checkout: "/w",
  });
  expect(String(body.request_id).length).toBeGreaterThanOrEqual(16);
  // The one thing a pad must never be: input appended to a session somebody is using.
  expect(writes()).toHaveLength(0);
});

it("retries a press under the id it already has, and gives a different press its own", async () => {
  // A failed call is not proof that nothing ran - the answer may simply have been lost.
  // Pressing again under a fresh id is how a command gets run twice, so the id stays
  // until the server answers. A press that differs in any of the bytes it was confirmed
  // against is a different press, and gets an id of its own - which is also the rule the
  // server refuses a reused id by, so the two can never disagree.
  server.pads = [
    { id: 3, position: 0, revision: 4, label: "Check", command: "cargo test" },
    { id: 4, position: 1, revision: 1, label: "Lint", command: "cargo clippy" },
  ];
  server.runFails = "Connection lost";
  render(<TerminalWorkspace project="widget" node={null} />);

  await run();
  await run();
  await waitFor(() => expect(posted("/terminals/commands")).toHaveLength(2));
  expect(pressBody(1).request_id).toBe(pressBody(0).request_id);
  expect(pressBody(1).revision).toBe(4);

  await run(/^Lint/);
  await waitFor(() => expect(posted("/terminals/commands")).toHaveLength(3));
  expect(pressBody(2).pad).toBe(4);
  expect(pressBody(2).request_id).not.toBe(pressBody(0).request_id);
});

it("keeps no id once a press has been answered", async () => {
  // The id survives an unanswered call, not an answered one: two deliberate presses of
  // the same pad are two runs, and must not be collapsed into one by a stale id.
  server.pads = [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" }];
  render(<TerminalWorkspace project="widget" node={null} />);

  await run();
  await run();
  await waitFor(() => expect(posted("/terminals/commands")).toHaveLength(2));
  expect(pressBody(1).request_id).not.toBe(pressBody(0).request_id);
});

it("shows the receipt a replayed press already had rather than reporting a second run", async () => {
  server.pads = [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" }];
  server.runReplay = { replayed: true, state: "finished", finished_at: "b", exit_code: 0 };
  render(<TerminalWorkspace project="widget" node={null} />);

  await run();
  expect((await screen.findByRole("alert")).textContent).toContain("already sent");
  expect(posted("/terminals/commands")).toHaveLength(1);
});

it("says that a replayed press with no recorded start is not being run again", async () => {
  // The one answer nobody can improve on: it may have run, and guessing either way is
  // worse than saying so.
  server.pads = [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" }];
  server.runReplay = { replayed: true, state: "uncertain", dispatch: "intent", session_id: null };
  render(<TerminalWorkspace project="widget" node={null} />);

  await run();
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("whether it started was never recorded");
  expect(alert.textContent).toContain("not being run again");
});

it("says a press that started nothing started nothing, and lets the next one be made", async () => {
  // A refusal after the press was recorded comes back as that row, not as a thrown
  // error: the window learns its press was answered and gives the next one an id of its
  // own, so a condition that has cleared - an update that finished - is not a press that
  // can never be made again.
  server.pads = [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" }];
  server.runReplay = { state: "refused", dispatch: "refused", session_id: null, refusal: "an update is installing" };
  render(<TerminalWorkspace project="widget" node={null} />);

  await run();
  expect((await screen.findByRole("alert")).textContent).toContain("an update is installing");

  server.runReplay = null;
  await run();
  await waitFor(() => expect(posted("/terminals/commands")).toHaveLength(2));
  expect(pressBody(1).request_id).not.toBe(pressBody(0).request_id);
});

it("will not press a pad until the deck has said which checkout it is on", async () => {
  // "The current one" is not a confirmation, and a project's name is not a directory.
  server.pads = [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" }];
  server.deckFails = true;
  render(<TerminalWorkspace project="widget" node={null} />);

  const pad = await screen.findByRole("button", { name: /^Check/ });
  expect(pad.getAttribute("disabled")).not.toBeNull();
  await act(async () => {
    fireEvent.click(pad);
  });
  expect(screen.queryByRole("dialog", { name: "Run this command" })).toBeNull();
  expect(posted("/terminals/commands")).toHaveLength(0);
});

it("says why a refused press ran nothing", async () => {
  server.pads = [{ id: 3, position: 0, revision: 1, label: "Check", command: "cargo test" }];
  server.runFails = "this pad now runs `cargo test --release`, not what you confirmed; nothing was run";
  render(<TerminalWorkspace project="widget" node={null} />);

  await run();
  expect((await screen.findByRole("alert")).textContent).toContain("nothing was run");
});

it("shows an exit and a duration only from a receipt, and never calls an unknown outcome a pass", async () => {
  server.deck = {
    worktree: "/w",
    running: [],
    history: [
      { id: 2, token: "t2", label: "Check", command: "cargo test", worktree: "/w", session_id: 8, started_at: "a", finished_at: "b", exit_code: 101, duration_ms: 4_200, lines: 90, lines_exact: false, state: "finished" },
      { id: 1, token: "t1", label: "Lint", command: "cargo clippy", worktree: "/w", session_id: 7, started_at: "a", finished_at: null, exit_code: null, duration_ms: null, lines: null, lines_exact: true, state: "unknown" },
    ],
  };
  render(<TerminalWorkspace project="widget" node={null} />);

  await waitFor(() => expect(screen.getAllByText("cargo test").length).toBeGreaterThan(0));
  expect(screen.getByText("101")).toBeDefined();
  expect(screen.getByText("4.2s")).toBeDefined();
  expect(screen.getByText("90+"), "a floor says so rather than reading as a total").toBeDefined();
  expect(screen.getAllByText(/Outcome not recorded/).length).toBeGreaterThan(0);
});

it("falls back to the standard renderer when WebGL is unavailable", async () => {
  xterm.webgl = "throws";
  render(<TerminalWorkspace project="widget" node={null} />);
  const tab = await screen.findByRole("tab", { name: /shell 1/ });
  await act(async () => {
    fireEvent.click(tab);
  });
  expect((await screen.findByText(/WebGL is unavailable here/)).textContent).toContain("standard renderer");
});

it("falls back when the GPU takes the context back mid-session", async () => {
  render(<TerminalWorkspace project="widget" node={null} />);
  const tab = await screen.findByRole("tab", { name: /shell 1/ });
  await act(async () => {
    fireEvent.click(tab);
  });
  await waitFor(() => expect(screen.getByText(/Renderer: WebGL/)).toBeDefined());
  await act(async () => {
    xterm.contextLoss?.();
  });
  expect(screen.getByText(/the standard renderer took over/)).toBeDefined();
  expect(xterm.webglDisposed).toBe(1);
});

it("attaches a selection to the draft and sends nothing", async () => {
  const sent = vi.fn();
  render(<TerminalWorkspace project="widget" node={null} onSend={sent} />);
  const tab = await screen.findByRole("tab", { name: /shell 1/ });
  await act(async () => {
    fireEvent.click(tab);
  });
  await act(async () => {
    xterm.selection = "error[E0425]: cannot find value `x`";
    xterm.selectionChanged();
  });
  expect(screen.getByText("1 line")).toBeDefined();
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Send to agent" }));
  });
  expect(sent).toHaveBeenCalledWith("error[E0425]: cannot find value `x`");
  // Attaching is not sending, and it is certainly not an instruction to the agent.
  expect(postedUnder("/chats")).toHaveLength(0);
});

it("says when output fell out of the scrollback instead of showing a seam", async () => {
  server.chunks = [{ text: "tail", cursor: 4_100, done: false, status: null, dropped: 4_096 }];
  render(<TerminalWorkspace project="widget" node={null} />);
  const tab = await screen.findByRole("tab", { name: /shell 1/ });
  await act(async () => {
    fireEvent.click(tab);
  });
  expect((await screen.findByText(/fell out of the scrollback/)).textContent).toContain("4,096 bytes");
});

it("stops queued input after an uncertain write and never resends it", async () => {
  server.writeFails = true;
  render(<TerminalWorkspace project="widget" node={null} />);
  const tab = await screen.findByRole("tab", { name: /shell 1/ });
  await act(async () => {
    fireEvent.click(tab);
  });
  await waitFor(() => expect(xterm.input).not.toBeNull());
  await act(async () => {
    xterm.input?.("rm -rf something");
    xterm.input?.("\r");
  });
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("not retried");
  await waitFor(() => expect(writes()).toHaveLength(1));
  await act(async () => {
    xterm.input?.("more");
  });
  expect(writes(), "the Enter behind a failed write is never delivered later").toHaveLength(1);
});

it("keeps the caret solid when the operator asked for less motion", async () => {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("prefers-reduced-motion"),
    media: query,
    addEventListener() {},
    removeEventListener() {},
  }));
  render(<TerminalWorkspace project="widget" node={null} />);
  const tab = await screen.findByRole("tab", { name: /shell 1/ });
  await act(async () => {
    fireEvent.click(tab);
  });
  await waitFor(() => expect(xterm.instances).toBeGreaterThan(0));
  expect(xterm.options.cursorBlink).toBe(false);
});
