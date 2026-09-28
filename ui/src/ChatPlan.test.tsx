import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ChatPlanning } from "./ChatPlan";
import type { ChatPlan } from "./plan-api";

const service = vi.hoisted(() => ({
  chatPlan: vi.fn(),
  changeChatPlan: vi.fn(),
}));
vi.mock("./plan-api", async (original) => ({
  ...(await original<typeof import("./plan-api")>()),
  ...service,
}));
const empty = (): ChatPlan => ({
  chat_id: 7,
  project_id: 1,
  revision: 0,
  bundle: null,
});
const plan = (): ChatPlan => ({
  ...empty(),
  revision: 4,
  bundle: {
    plan: {
      id: 2,
      slug: "chat-7",
      title: "Improve search",
      summary: "Fast and accessible",
      status: "ready",
    },
    sections: [{ key: "scope", title: "Scope", body: "Only search", rev: 1 }],
    slices: [
      {
        id: 5,
        key: "S1",
        title: "Search field",
        scope_md: "Wire search\n\nTouches: src/**",
        demo_md: "Run tests",
        status: "ready",
        blocked_reason: null,
        rev: 1,
      },
    ],
    questions: [
      {
        id: 9,
        body: "Which search mode?",
        status: "open",
        answer: null,
        slice_key: "S1",
      },
    ],
    log: [],
    decisions: [],
    gotchas: [],
  },
});
const props = { chatId: 7, tick: 0, archived: false, onChanged: vi.fn() };

beforeEach(() => {
  vi.clearAllMocks();
  service.chatPlan.mockResolvedValue(empty());
});

describe("chat-owned planning", () => {
  it("does not create a plan simply by opening Overview", async () => {
    render(<ChatPlanning {...props} />);
    expect(await screen.findByText(/No plan yet/)).toBeTruthy();
    expect(service.changeChatPlan).not.toHaveBeenCalled();
    service.changeChatPlan.mockResolvedValue(plan());
    fireEvent.change(screen.getByLabelText("Plan title"), {
      target: { value: "Improve search" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create plan" }));
    expect(await screen.findByText("Improve search")).toBeTruthy();
    expect(service.changeChatPlan).toHaveBeenCalledWith(
      7,
      { action: "create_plan", title: "Improve search", summary: undefined },
      0,
    );
    expect(
      screen.getByLabelText("Reported slice progress").getAttribute("value"),
    ).toBe("0");
  });

  it("does not fill Overview with the engine's empty outline placeholders", async () => {
    const initial = plan();
    initial.bundle!.sections = [
      { key: "outcome", title: "Outcome", body: "", rev: 1 },
    ];
    service.chatPlan.mockResolvedValue(initial);
    render(<ChatPlanning {...props} />);
    await screen.findByText("Improve search");
    expect(screen.queryByRole("heading", { name: "Outcome" })).toBeNull();
  });

  it("answers the displayed chat's question, retaining the draft on conflict", async () => {
    service.chatPlan.mockResolvedValue(plan());
    service.changeChatPlan.mockRejectedValueOnce(
      new Error("The plan changed; refresh and retry"),
    );
    render(<ChatPlanning {...props} />);
    const answer = await screen.findByLabelText("Your answer");
    fireEvent.change(answer, { target: { value: "Use prefix matching" } });
    fireEvent.click(screen.getByRole("button", { name: "Answer question" }));
    expect((await screen.findByRole("alert")).textContent).toContain("changed");
    expect((answer as HTMLTextAreaElement).value).toBe("Use prefix matching");
    expect(service.changeChatPlan).toHaveBeenCalledWith(
      7,
      {
        action: "answer_question",
        question_id: 9,
        answer: "Use prefix matching",
      },
      4,
    );
    const answered = plan();
    answered.revision = 5;
    answered.bundle!.questions[0] = {
      ...answered.bundle!.questions[0]!,
      status: "answered",
      answer: "Use prefix matching",
    };
    service.changeChatPlan.mockResolvedValue(answered);
    fireEvent.click(screen.getByRole("button", { name: "Answer question" }));
    expect(await screen.findByText("Answered questions (1)")).toBeTruthy();
    expect(screen.queryByLabelText("Your answer")).toBeNull();
  });

  it("keeps an editor's base revision when new planning evidence arrives", async () => {
    service.chatPlan.mockResolvedValue(plan());
    const view = render(<ChatPlanning {...props} />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Scope" }));
    fireEvent.change(screen.getByLabelText("Content"), {
      target: { value: "My unsaved scope" },
    });
    const newer = plan();
    newer.revision = 8;
    newer.bundle!.sections[0]!.body = "Another writer's scope";
    service.chatPlan.mockResolvedValue(newer);
    view.rerender(<ChatPlanning {...props} tick={1} />);
    expect(
      await screen.findByText(/changed while you were editing/),
    ).toBeTruthy();
    expect(
      (screen.getByLabelText("Content") as HTMLTextAreaElement).value,
    ).toBe("My unsaved scope");
    service.changeChatPlan.mockRejectedValue(new Error("stale"));
    fireEvent.click(screen.getByRole("button", { name: "Save section" }));
    await waitFor(() =>
      expect(service.changeChatPlan).toHaveBeenCalledWith(
        7,
        {
          action: "write_section",
          key: "scope",
          title: "Scope",
          body: "My unsaved scope",
        },
        4,
      ),
    );
    await screen.findByRole("alert");
  });

  it("does not let an older in-flight refresh erase a successful write", async () => {
    service.chatPlan.mockResolvedValueOnce(empty());
    const view = render(<ChatPlanning {...props} />);
    await screen.findByLabelText("Plan title");
    let resolve!: (value: ChatPlan) => void;
    service.chatPlan.mockImplementationOnce(
      () =>
        new Promise<ChatPlan>((done) => {
          resolve = done;
        }),
    );
    view.rerender(<ChatPlanning {...props} tick={1} />);
    service.changeChatPlan.mockResolvedValue(plan());
    fireEvent.change(screen.getByLabelText("Plan title"), {
      target: { value: "Improve search" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create plan" }));
    await screen.findByText("Improve search");
    await act(async () => resolve(empty()));
    expect(screen.getByText("Improve search")).toBeTruthy();
  });

  it("clears a transient read error after the next successful refresh", async () => {
    service.chatPlan.mockRejectedValueOnce(
      new Error("Temporarily unavailable"),
    );
    const view = render(<ChatPlanning {...props} />);
    await screen.findByRole("alert");
    service.chatPlan.mockResolvedValue(plan());
    view.rerender(<ChatPlanning {...props} tick={1} />);
    await screen.findByText("Improve search");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("discards a late response after switching to another keyed chat", async () => {
    let resolve!: (value: ChatPlan) => void;
    service.chatPlan.mockImplementationOnce(
      () =>
        new Promise<ChatPlan>((done) => {
          resolve = done;
        }),
    );
    const view = render(<ChatPlanning key={7} {...props} />);
    const other = plan();
    other.chat_id = 8;
    other.bundle!.plan.title = "Other chat's plan";
    service.chatPlan.mockResolvedValue(other);
    view.rerender(<ChatPlanning key={8} {...props} chatId={8} />);
    await screen.findByText("Other chat's plan");
    await act(async () => resolve(plan()));
    expect(screen.queryByText("Improve search")).toBeNull();
    expect(service.chatPlan).toHaveBeenLastCalledWith(8);
  });

  it("requires a blocking reason and sends only the selected slice", async () => {
    service.chatPlan.mockResolvedValue(plan());
    service.changeChatPlan.mockResolvedValue(plan());
    render(<ChatPlanning {...props} />);
    fireEvent.change(await screen.findByLabelText("S1 status"), {
      target: { value: "blocked" },
    });
    expect(
      (screen.getByLabelText("Blocking reason") as HTMLInputElement).required,
    ).toBe(true);
    fireEvent.change(screen.getByLabelText("Blocking reason"), {
      target: { value: "Need an answer" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update status" }));
    await waitFor(() =>
      expect(service.changeChatPlan).toHaveBeenCalledWith(
        7,
        {
          action: "set_slice_status",
          key: "S1",
          status: "blocked",
          reason: "Need an answer",
        },
        4,
      ),
    );
  });
});
