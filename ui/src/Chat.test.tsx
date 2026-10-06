import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ChatView } from "./Chat";
import type { ChatDetail } from "./chat-api";
import type { Project, RunEvent } from "./api";

const service = vi.hoisted(() => ({
  chat: vi.fn(),
  chatEvents: vi.fn(),
  createChat: vi.fn(),
  sendChat: vi.fn(),
  stopChat: vi.fn(),
  resumeChat: vi.fn(),
  renameChat: vi.fn(),
  archiveChat: vi.fn(),
  setChatMode: vi.fn(),
  queueChatFollowup: vi.fn(),
  cancelChatFollowup: vi.fn(),
  sendChatFollowup: vi.fn(),
  chatPlan: vi.fn(),
}));
vi.mock("./Editor", () => ({ Editor: ({ project, workspace, visible }: { project: string; workspace: string; visible: boolean }) => <div><p>Editor {project}: {workspace} ({String(visible)})</p><input aria-label="Unsaved fixture file" defaultValue="kept" /></div> }));
vi.mock("./Terminal", () => ({ TerminalPane: ({ project, workspace }: { project: string; workspace: string }) => <p>Terminal {project}: {workspace}</p> }));
vi.mock("./ChatChanges", () => ({ ChatChanges: ({ chatId, onFeedback }: { chatId: number; onFeedback: (text: string) => void }) => <section aria-label="Review fixture"><p>Review chat {chatId}</p><input aria-label="Unsaved review finding" /><button onClick={() => onFeedback("Recorded feedback")}>Use feedback</button></section> }));
vi.mock("./chat-api", () => service);
vi.mock("./team-api", async (original) => ({
  ...(await original<typeof import("./team-api")>()),
  setChatMode: service.setChatMode,
}));
vi.mock("./plan-api", async (original) => ({
  ...(await original<typeof import("./plan-api")>()),
  chatPlan: service.chatPlan,
}));
vi.mock("./api", async (original) => ({
  ...(await original<typeof import("./api")>()),
  models: () =>
    Promise.resolve({
      models: [
        {
          provider: "openai",
          runtime_provider: "openai-codex",
          model: "gpt-5",
          context: "200k",
          context_tokens: 200000,
          max_output: 10000,
          thinking: true,
          images: true,
        },
      ],
      error: null,
    }),
}));

const project: Project = {
  id: 1,
  slug: "demo",
  name: "Demo",
  kind: "repo",
  status: "active",
  open_runs: 0,
};
const detail = (id = 1): ChatDetail => ({
  id,
  mode: "single",
  team_builds: [],
  project_id: 1,
  title: `Chat ${id}`,
  workspace_path: "/repo/demo",
  provider: "openai",
  model: "gpt-5",
  reasoning: "high",
  active_node_id: null,
  live_text: "",
  stop_requested: false,
  archived: false,
  rev: 0,
  created_at: "2026-01-01",
  updated_at: "2026-01-01",
  turns: [],
  followups: [],
  state: "idle",
  can_resume: false,
  orphan_running: false,
});
const event = (id: number, message: string, actor = "human"): RunEvent => ({
  id,
  node_run_id: 1,
  kind: "note",
  at: "2026-01-01",
  summary: message,
  message,
  thinking: [],
  actor,
});
const props = () => ({
  id: 1 as number | null,
  project,
  tick: 0,
  onCreated: vi.fn(),
  onChanged: vi.fn(),
  onArchived: vi.fn(),
  onSettings: vi.fn(),
});

beforeEach(() => {
  vi.clearAllMocks();
  service.chat.mockImplementation((id: number) => Promise.resolve(detail(id)));
  service.chatEvents.mockResolvedValue([]);
  service.chatPlan.mockImplementation((id: number) => Promise.resolve({ chat_id: id, project_id: 1, revision: 0, bundle: null }));
  service.createChat.mockResolvedValue(detail(3));
  service.sendChat.mockResolvedValue({ run_id: 1, node_id: 1, started: true });
});

describe("persistent conversation", () => {
  it("keeps conversation and composer alongside chat-owned overview, review, board and work", async () => {
    service.chatEvents.mockResolvedValue([event(1, "Keep this conversation visible")]);
    render(<ChatView {...props()} id={2} />);
    await screen.findByRole("heading", { name: "Chat 2" });
    expect(screen.queryByRole("tablist", { name: "Chat views" })).toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Unsent instruction" } });
    for (const panel of ["Overview", "Review", "Board", "Work"]) {
      fireEvent.click(screen.getByRole("button", { name: panel }));
      await screen.findByRole("region", { name: panel });
      expect(screen.getByRole("article", { name: "You" }).textContent).toContain("Keep this conversation visible");
      expect((screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement).value).toBe("Unsent instruction");
    }
    await waitFor(() => expect(service.chatPlan).toHaveBeenCalledWith(2));
    expect(service.sendChat).not.toHaveBeenCalled();
  });

  it("keeps review drafts across panels and adds feedback without sending it", async () => {
    render(<ChatView {...props()} id={2} />);
    await screen.findByRole("heading", { name: "Chat 2" });
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    await screen.findByText("Review chat 2");
    fireEvent.change(screen.getByRole("textbox", { name: "Unsaved review finding" }), { target: { value: "Keep this draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    await screen.findByRole("region", { name: "Chat plan" });
    expect(screen.queryByRole("textbox", { name: "Unsaved review finding" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect((screen.getByRole("textbox", { name: "Unsaved review finding" }) as HTMLInputElement).value).toBe("Keep this draft");
    fireEvent.click(screen.getByRole("button", { name: "Use feedback" }));
    expect((screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement).value).toBe("Recorded feedback");
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Message" }));
    fireEvent.keyDown(screen.getByRole("region", { name: "Review" }), { key: "Escape" });
    expect(screen.queryByRole("region", { name: "Review" })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Review" }));
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect((screen.getByRole("textbox", { name: "Unsaved review finding" }) as HTMLInputElement).value).toBe("Keep this draft");
    expect(service.sendChat).not.toHaveBeenCalled();
  });
  it("uses the composer action to stop, or queue when typing, without making Enter stop a turn", async () => {
    service.chat.mockResolvedValue({ ...detail(), state: "running", active_node_id: 41 });
    const input = props();
    const view = render(<ChatView {...input} />);
    const stop = await screen.findByRole("button", { name: "Stop turn" });
    expect(stop.closest(".chat-composer")).not.toBeNull();
    expect(stop.getAttribute("data-working")).toBe("true");
    expect(within(screen.getByRole("group", { name: "Chat tools" })).queryByRole("button", { name: "Stop turn" })).toBeNull();
    const message = screen.getByRole("textbox", { name: "Message" });
    fireEvent.keyDown(message, { key: "Enter" });
    expect(service.stopChat).not.toHaveBeenCalled();
    fireEvent.change(message, { target: { value: "Check the error path too" } });
    expect(screen.queryByRole("button", { name: "Stop turn" })).toBeNull();
    const queue = screen.getByRole("button", { name: "Queue follow-up" });
    expect(queue.closest(".chat-composer")).not.toBeNull();
    expect((queue as HTMLButtonElement).disabled).toBe(false);
    fireEvent.change(message, { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "Stop turn" }));
    await waitFor(() => expect(service.stopChat).toHaveBeenCalledWith(1, 41));
    service.chat.mockResolvedValue({ ...detail(), state: "stopped" });
    view.rerender(<ChatView {...input} tick={1} />);
    const send = await screen.findByRole("button", { name: "Send message" });
    expect(send.getAttribute("data-working")).toBe("false");
    expect((send as HTMLButtonElement).disabled).toBe(true);
    expect(service.sendChat).not.toHaveBeenCalled();
    expect(service.queueChatFollowup).not.toHaveBeenCalled();
  });

  it("keeps stopping and interrupted recovery in the composer without retargeting a typed draft", async () => {
    service.chat.mockResolvedValue({ ...detail(), state: "stopping", stop_requested: true, active_node_id: 41 });
    const input = props();
    const view = render(<ChatView {...input} />);
    const stopping = await screen.findByRole("button", { name: "Stopping…" });
    expect(stopping.closest(".chat-composer")).not.toBeNull();
    expect((stopping as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(stopping);
    expect(service.stopChat).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Keep this draft" } });
    service.chat.mockResolvedValue({ ...detail(), state: "interrupted", active_node_id: 41, can_resume: true });
    view.rerender(<ChatView {...input} tick={1} />);
    const stop = await screen.findByRole("button", { name: "Stop turn" });
    expect(stop.getAttribute("data-working")).toBe("false");
    fireEvent.click(stop);
    await waitFor(() => expect(service.stopChat).toHaveBeenCalledWith(1, 41));
    expect((screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement).value).toBe("Keep this draft");
    expect(service.queueChatFollowup).not.toHaveBeenCalled();
  });

  it("queues against the exact solo turn rather than pretending to send into a running process", async () => {
    service.chat.mockResolvedValue({ ...detail(), state: "running", active_node_id: 41 });
    render(<ChatView {...props()} />);
    await screen.findByRole("heading", { name: "Chat 1" });
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Check the error path too" } });
    fireEvent.click(screen.getByRole("button", { name: "Queue follow-up" }));
    await waitFor(() => expect(service.queueChatFollowup).toHaveBeenCalledWith(1, 41, "Check the error path too", expect.any(String), "follow_up"));
    expect(service.sendChat).not.toHaveBeenCalled();
    expect(service.stopChat).not.toHaveBeenCalled();
  });

  it("makes stop-and-steer one exact request and preserves its identity after a lost response", async () => {
    service.chat.mockResolvedValue({ ...detail(), state: "running", active_node_id: 41 });
    service.queueChatFollowup.mockRejectedValueOnce(new Error("Connection lost"));
    render(<ChatView {...props()} />);
    await screen.findByRole("heading", { name: "Chat 1" });
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Use the other implementation" } });
    fireEvent.click(screen.getByRole("button", { name: "Stop and steer" }));
    await screen.findByRole("alert");
    const request = service.queueChatFollowup.mock.calls[0];
    expect(request).toEqual([1, 41, "Use the other implementation", expect.any(String), "steer"]);
    fireEvent.click(screen.getByRole("button", { name: "Stop and steer" }));
    await waitFor(() => expect(service.queueChatFollowup).toHaveBeenCalledTimes(2));
    expect(service.queueChatFollowup.mock.calls[1]).toEqual(request);
    expect(service.stopChat).not.toHaveBeenCalled();
  });

  it("never retargets a lost queue response to the next active turn", async () => {
    service.chat.mockResolvedValue({ ...detail(), state: "running", active_node_id: 41 });
    service.queueChatFollowup.mockRejectedValueOnce(new Error("Connection lost"));
    const input = props();
    const view = render(<ChatView {...input} />);
    await screen.findByRole("heading", { name: "Chat 1" });
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Exact instruction" } });
    fireEvent.click(screen.getByRole("button", { name: "Queue follow-up" }));
    await screen.findByRole("alert");
    const request = service.queueChatFollowup.mock.calls[0];
    service.chat.mockResolvedValue({ ...detail(), state: "running", active_node_id: 42 });
    view.rerender(<ChatView {...input} tick={1} />);
    await waitFor(() => expect(service.chat).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole("button", { name: "Queue follow-up" }));
    await waitFor(() => expect(service.queueChatFollowup).toHaveBeenCalledTimes(2));
    expect(service.queueChatFollowup.mock.calls[1]).toEqual(request);
  });

  it("offers finishing the stop, not resuming the old prompt, after an interrupted steer", async () => {
    service.chat.mockResolvedValue({ ...detail(), state: "interrupted", active_node_id: 41, can_resume: true, stop_requested: true, followups: [{ id: 8, chat_id: 1, after_node_id: 41, body: "Do this instead", kind: "steer", state: "queued", node_id: null, created_at: "2026-01-01", delivered_at: null }] });
    render(<ChatView {...props()} />);
    await screen.findByRole("button", { name: "Finish stop and send steering" });
    const stop = screen.getByRole("button", { name: "Stop turn" });
    expect((stop as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(stop);
    await waitFor(() => expect(service.stopChat).toHaveBeenCalledWith(1, 41));
    expect(service.resumeChat).not.toHaveBeenCalled();
  });

  it("does not offer retry for a claimed instruction without a delivery acknowledgement", async () => {
    service.chat.mockResolvedValue({ ...detail(), followups: [{ id: 8, chat_id: 1, after_node_id: 41, body: "Uncertain instruction", kind: "follow_up", state: "starting", node_id: 42, created_at: "2026-01-01", delivered_at: null }] });
    render(<ChatView {...props()} />);
    await screen.findByText(/Delivery unconfirmed/);
    expect(screen.queryByRole("button", { name: "Send queued message" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel queued message" })).toBeNull();
    expect(service.sendChatFollowup).not.toHaveBeenCalled();
  });

  it("shows a held queue after restart without sending it, and cancels its exact receipt", async () => {
    service.chat.mockResolvedValue({ ...detail(), followups: [{ id: 8, chat_id: 1, after_node_id: 41, body: "Check the error path too", kind: "follow_up", state: "queued", node_id: null, created_at: "2026-01-01", delivered_at: null }] });
    render(<ChatView {...props()} />);
    await screen.findByText("Check the error path too");
    expect(screen.getByText(/Queued · not delivered/)).not.toBeNull();
    expect(service.sendChatFollowup).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel queued message" }));
    await waitFor(() => expect(service.cancelChatFollowup).toHaveBeenCalledWith(1, 8));
    expect(service.sendChat).not.toHaveBeenCalled();
  });

  it("opens tools only on request in this chat's checkout, keeping drafts and buffers across toggles", async () => {
    service.chat.mockResolvedValue({ ...detail(), workspace_path: "/repo/linked-chat" });
    render(<ChatView {...props()} />);
    await screen.findByRole("heading", { name: "Chat 1" });
    expect(screen.queryByText(/Terminal demo:/)).toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Unsent message" } });
    fireEvent.click(screen.getByRole("button", { name: "Editor" }));
    await screen.findByText("Editor demo: /repo/linked-chat (true)");
    fireEvent.change(screen.getByRole("textbox", { name: "Unsaved fixture file" }), { target: { value: "Uncommitted edit" } });
    fireEvent.click(screen.getByRole("button", { name: "Terminal" }));
    await screen.findByText("Terminal demo: /repo/linked-chat");
    expect(screen.getByText("Editor demo: /repo/linked-chat (false)")).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Editor" }));
    expect((screen.getByRole("textbox", { name: "Unsaved fixture file" }) as HTMLInputElement).value).toBe("Uncommitted edit");
    expect((screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement).value).toBe("Unsent message");
    expect(service.sendChat).not.toHaveBeenCalled();
    expect(screen.getByText(/not covered by the agent's checkout guard/)).not.toBeNull();
  });

  it("does not guess a checkout for tools before the chat exists", async () => {
    render(<ChatView {...props()} id={null} />);
    await screen.findByRole("option", { name: "gpt-5 · openai" });
    expect(screen.queryByRole("button", { name: "Editor" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Terminal" })).toBeNull();
  });

  it("shows an attachment recovery failure without offering an automatic restart", async () => {
    service.chat.mockResolvedValue({ ...detail(), recovery_error: "Could not inspect child_epoch" });
    render(<ChatView {...props()} />);
    expect((await screen.findByRole("alert")).textContent).toContain("Could not inspect child_epoch");
    expect(screen.getByRole("alert").textContent).toContain("Restart AI Team");
    expect(service.resumeChat).not.toHaveBeenCalled();
    expect(service.sendChat).not.toHaveBeenCalled();
  });

  it("retries a terminal tick that arrives during an in-flight detail request", async () => {
    let resolve!: (value: ChatDetail) => void;
    service.chat.mockImplementationOnce(
      () =>
        new Promise<ChatDetail>((done) => {
          resolve = done;
        }),
    );
    const input = props();
    const view = render(<ChatView {...input} />);
    await waitFor(() => expect(service.chat).toHaveBeenCalledOnce());
    view.rerender(<ChatView {...input} tick={1} />);
    await act(async () =>
      resolve({ ...detail(), state: "running", active_node_id: 10 }),
    );
    await waitFor(() => expect(service.chat).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("button", { name: "Stop turn" })).toBeNull();
  });

  it("creates a solo chat and sends the first message without using legacy team Start", async () => {
    const input = { ...props(), id: null };
    render(<ChatView {...input} />);
    await screen.findByRole("option", { name: "gpt-5 · openai" });
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), {
      target: { value: "Explain the app" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await waitFor(() => expect(input.onCreated).toHaveBeenCalledWith(3));
    expect(service.createChat).toHaveBeenCalledWith({
      project: "demo",
      provider: "openai",
      model: "gpt-5",
      reasoning: "high",
    });
    expect(service.sendChat).toHaveBeenCalledWith(
      3,
      "Explain the app",
      expect.any(String),
    );
    expect(screen.queryByRole("button", { name: /Start team/ })).toBeNull();
  });

  it("selects Team before the first message without changing the saved solo model", async () => {
    render(<ChatView {...props()} id={null} />);
    await screen.findByRole("option", { name: "gpt-5 · openai" });
    fireEvent.change(screen.getByRole("combobox", { name: "Execution mode" }), { target: { value: "team" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Plan this with the team" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await waitFor(() => expect(service.createChat).toHaveBeenCalledWith({ project: "demo", provider: "openai", model: "gpt-5", reasoning: "high", mode: "team" }));
  });

  it("switches the existing idle chat with its exact revision, not a new chat", async () => {
    render(<ChatView {...props()} />);
    await screen.findByRole("heading", { name: "Chat 1" });
    fireEvent.change(screen.getByRole("combobox", { name: "Execution mode" }), { target: { value: "team" } });
    await waitFor(() => expect(service.setChatMode).toHaveBeenCalledWith(1, "team", 0));
    expect(service.createChat).not.toHaveBeenCalled();
    expect(service.sendChat).not.toHaveBeenCalled();
  });

  it("retains the draft and request identity when a response is lost", async () => {
    service.sendChat.mockRejectedValueOnce(new Error("Connection lost"));
    render(<ChatView {...props()} />);
    await screen.findByRole("heading", { name: "Chat 1" });
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), {
      target: { value: "Continue" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await screen.findByRole("alert");
    expect(
      (screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement)
        .value,
    ).toBe("Continue");
    const request = service.sendChat.mock.calls[0];
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await waitFor(() => expect(service.sendChat).toHaveBeenCalledTimes(2));
    expect(service.sendChat.mock.calls[1]).toEqual(request);
    await waitFor(() =>
      expect(
        (
          screen.getByRole("textbox", {
            name: "Message",
          }) as HTMLTextAreaElement
        ).value,
      ).toBe(""),
    );
  });

  it("reads every event page and appends new events without losing older messages", async () => {
    service.chatEvents.mockImplementation((_id: number, after: number) =>
      Promise.resolve(
        after === 0
          ? Array.from({ length: 500 }, (_, index) =>
              event(index + 1, `Message ${index + 1}`),
            )
          : after === 500
            ? [event(501, "A persisted answer", "agent")]
            : [],
      ),
    );
    const input = props();
    const view = render(<ChatView {...input} />);
    await screen.findByText("A persisted answer");
    expect(service.chatEvents).toHaveBeenCalledWith(1, 500);
    view.rerender(<ChatView {...input} tick={1} />);
    await waitFor(() =>
      expect(service.chatEvents).toHaveBeenCalledWith(1, 501),
    );
    expect(screen.getAllByText("Message 1")).toHaveLength(1);
  });

  it("never paints an old chat's late response into a newly selected chat", async () => {
    let resolve!: (value: ChatDetail) => void;
    service.chat.mockImplementation((id: number) =>
      id === 1
        ? new Promise<ChatDetail>((done) => {
            resolve = done;
          })
        : Promise.resolve(detail(id)),
    );
    service.chatEvents.mockImplementation((id: number) =>
      Promise.resolve([event(id, `History ${id}`)]),
    );
    const input = props();
    const view = render(<ChatView key={1} {...input} />);
    await waitFor(() => expect(service.chat).toHaveBeenCalledWith(1));
    view.rerender(<ChatView key={2} {...input} id={2} />);
    await screen.findByText("History 2");
    await act(async () => resolve(detail(1)));
    expect(screen.queryByText("History 1")).toBeNull();
    expect(screen.getByRole("heading", { name: "Chat 2" })).not.toBeNull();
  });

  it("keeps stop commands scoped to this chat and this exact active node", async () => {
    service.chat.mockResolvedValue({
      ...detail(2),
      active_node_id: 19,
      state: "running",
    });
    render(<ChatView {...props()} id={2} />);
    await screen.findByRole("button", { name: "Stop turn" });
    fireEvent.click(screen.getByRole("button", { name: "Overview" }));
    fireEvent.click(screen.getByRole("button", { name: "Stop turn" }));
    await waitFor(() => expect(service.stopChat).toHaveBeenCalledWith(2, 19));
    expect(screen.queryByRole("button", { name: "Queue follow-up" })).toBeNull();
    expect(screen.getByRole("button", { name: "Stop turn" }).closest(".chat-composer")).not.toBeNull();
    expect(
      screen
        .getByRole("progressbar", { name: "Agent working" })
        .hasAttribute("aria-valuenow"),
    ).toBe(false);
  });

  it("offers recovery only after the supervisor and child have both gone", async () => {
    service.chat.mockResolvedValue({
      ...detail(),
      active_node_id: 11,
      state: "interrupted",
      can_resume: true,
    });
    const view = render(<ChatView key="dead" {...props()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Resume turn" }));
    await waitFor(() => expect(service.resumeChat).toHaveBeenCalledWith(1, 11));
    service.chat.mockResolvedValue({
      ...detail(),
      active_node_id: 11,
      state: "interrupted",
      can_resume: false,
      orphan_running: true,
    });
    view.rerender(<ChatView key="orphan" {...props()} />);
    await screen.findByText(/original Pi process is still running/);
    expect(screen.queryByRole("button", { name: "Resume turn" })).toBeNull();
    expect(
      (screen.getByRole("button", { name: "Stop turn" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it("archives without deleting the transcript and notifies the shell", async () => {
    const input = props();
    render(<ChatView {...input} />);
    fireEvent.click(await screen.findByRole("button", { name: "Archive" }));
    await waitFor(() => expect(input.onArchived).toHaveBeenCalledOnce());
    expect(service.archiveChat).toHaveBeenCalledWith(1);
  });
});
