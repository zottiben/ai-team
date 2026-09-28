import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
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
}));
vi.mock("./chat-api", () => service);
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
  service.createChat.mockResolvedValue(detail(3));
  service.sendChat.mockResolvedValue({ run_id: 1, node_id: 1, started: true });
});

describe("persistent conversation", () => {
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
    fireEvent.click(screen.getByRole("tab", { name: "Overview" }));
    fireEvent.click(screen.getByRole("button", { name: "Stop turn" }));
    await waitFor(() => expect(service.stopChat).toHaveBeenCalledWith(2, 19));
    expect(
      (
        screen.getByRole("button", {
          name: "Send message",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
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
