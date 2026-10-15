import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ChatPlanning } from "./ChatPlan";
import type { ChatPlan } from "./plan-api";
import { useLayoutEffect } from "react";
import { SliceStatus } from "./PlanForms";
import { setSynth } from "./ui-preferences";

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
  setSynth(false);
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

  it("freezes approved work while leaving human questions available", async () => {
    service.chatPlan.mockResolvedValue(plan());
    render(<ChatPlanning {...props} frozen />);
    await screen.findByText("Improve search");
    expect((screen.getByRole("combobox", { name: "Plan status" }) as HTMLSelectElement).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Add slice" }).closest("fieldset")?.disabled).toBe(true);
    expect((screen.getByRole("textbox", { name: "Your answer" }) as HTMLTextAreaElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: "Ask a question" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("keeps answered history readable when the chat is archived", async () => {
    const archived = plan(); archived.bundle!.questions[0]!.status = "answered";
    archived.bundle!.questions[0]!.answer = "Recorded choice";
    service.chatPlan.mockResolvedValue(archived);
    render(<ChatPlanning {...props} archived/>);
    const history = await screen.findByRole("button", { name: "Answered questions (1)" });
    expect(history.matches(":disabled")).toBe(false);
    fireEvent.click(history);
    expect(await screen.findByText("Recorded choice")).toBeTruthy();
    expect(service.changeChatPlan).not.toHaveBeenCalled();
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

  it("retains separate section editors and their original field values", async () => {
    const initial = plan();
    initial.bundle!.sections.push({ key: "design", title: "Design", body: "Original design", rev: 1 });
    service.chatPlan.mockResolvedValue(initial);
    const view = render(<ChatPlanning {...props}/>);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Scope" }));
    const scope = screen.getByRole("textbox", { name: "Content" });
    fireEvent.change(scope, { target: { value: "My retained scope" } });
    fireEvent.click(screen.getByRole("button", { name: "Edit Design" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Content" }), { target: { value: "My retained design" } });
    fireEvent.click(screen.getByRole("button", { name: "Edit Scope" }));
    expect(screen.getByRole("textbox", { name: "Content" })).toBe(scope);
    expect((scope as HTMLTextAreaElement).value).toBe("My retained scope");
    const newer = structuredClone(initial); newer.revision = 8;
    newer.bundle!.sections[0]!.title = "Another writer's title";
    service.chatPlan.mockResolvedValue(newer);
    view.rerender(<ChatPlanning {...props} tick={1}/>);
    await screen.findByRole("button", { name: "Edit Another writer's title" });
    expect((screen.getByRole("textbox", { name: "Section title" }) as HTMLInputElement).value).toBe("Scope");
    fireEvent.click(screen.getByRole("button", { name: "Edit Design" }));
    expect((screen.getByRole("textbox", { name: "Content" }) as HTMLTextAreaElement).value).toBe("My retained design");
  });

  it("keeps one question editor through Synth subtabs and the legacy document", async () => {
    setSynth(true); service.chatPlan.mockResolvedValue(plan());
    render(<ChatPlanning {...props}/>);
    expect(await screen.findByRole("heading", { level: 2, name: "Improve search" })).toBeTruthy();
    const questions = await screen.findByRole("tab", { name: /^Open questions/ });
    fireEvent.click(questions);
    const answer = screen.getByRole("textbox", { name: "Your answer" });
    fireEvent.change(answer, { target: { value: "Retain across all views" } });
    expect(document.getElementById(questions.getAttribute("aria-controls")!)?.contains(answer)).toBe(true);
    fireEvent.click(screen.getByRole("tab", { name: "Plan" }));
    expect(answer.isConnected).toBe(true);
    act(() => setSynth(false));
    expect(screen.getByRole("textbox", { name: "Your answer" })).toBe(answer);
    act(() => setSynth(true));
    fireEvent.click(await screen.findByRole("tab", { name: /^Open questions/ }));
    expect(screen.getByRole("textbox", { name: "Your answer" })).toBe(answer);
    expect((answer as HTMLTextAreaElement).value).toBe("Retain across all views");
    expect(service.changeChatPlan).not.toHaveBeenCalled();
  });

  it("retains a Board drawer editor when switching back to the legacy document", async () => {
    setSynth(true);
    service.chatPlan.mockResolvedValue(plan());
    render(<ChatPlanning {...props}/>);
    fireEvent.click(await screen.findByRole("button", { name: "S1 Search field" }));
    const drawer = screen.getByRole("complementary", { name: "Slice S1" });
    fireEvent.click(within(drawer).getByRole("button", { name: "Edit S1" }));
    const scope = within(drawer).getByLabelText("Scope");
    fireEvent.change(scope, { target: { value: "Retained drawer work" } });
    act(() => setSynth(false));
    expect(scope.isConnected).toBe(true);
    expect(drawer.closest("[hidden]")).not.toBeNull();
    act(() => setSynth(true));
    expect(within(screen.getByRole("complementary", { name: "Slice S1" })).getByLabelText("Scope")).toBe(scope);
    expect((scope as HTMLTextAreaElement).value).toBe("Retained drawer work");
    expect(service.changeChatPlan).not.toHaveBeenCalled();
  });

  it("keeps an answer bound to the revision at which writing began", async () => {
    service.chatPlan.mockResolvedValue(plan());
    const view = render(<ChatPlanning {...props}/>);
    const answer = await screen.findByLabelText("Your answer");
    fireEvent.change(answer, { target: { value: "My original answer" } });
    service.chatPlan.mockResolvedValue({ ...plan(), revision: 8 });
    view.rerender(<ChatPlanning {...props} tick={1}/>);
    await waitFor(() => expect(service.chatPlan).toHaveBeenCalledTimes(2));
    await waitFor(() => expect((screen.getByRole("button", { name: "Answer question" }) as HTMLButtonElement).disabled).toBe(true));
    expect((answer as HTMLTextAreaElement).value).toBe("My original answer");
    expect(service.changeChatPlan).not.toHaveBeenCalled();
  });

  it("retains an unsent answer when another writer resolves its question", async () => {
    service.chatPlan.mockResolvedValue(plan());
    const view = render(<ChatPlanning {...props}/>);
    const answer = await screen.findByLabelText("Your answer");
    fireEvent.change(answer, { target: { value: "My unsent answer" } });
    const newer = plan(); newer.revision = 8;
    newer.bundle!.questions[0]!.status = "answered"; newer.bundle!.questions[0]!.answer = "A recorded answer";
    service.chatPlan.mockResolvedValue(newer);
    view.rerender(<ChatPlanning {...props} tick={1}/>);
    await screen.findByText("Answered questions (1)");
    expect(answer.isConnected).toBe(true);
    expect((answer as HTMLTextAreaElement).value).toBe("My unsent answer");
    expect((screen.getByRole("button", { name: "Answer question" }) as HTMLButtonElement).disabled).toBe(true);
    expect(service.changeChatPlan).not.toHaveBeenCalled();
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

  it("hides a previous chat synchronously and never compares revisions across owners", async () => {
    service.chatPlan.mockResolvedValue(plan());
    const view = render(<ChatPlanning {...props}/>);
    await screen.findByText("Improve search");
    let finish!: (value: ChatPlan) => void;
    service.chatPlan.mockImplementation(() => new Promise<ChatPlan>(resolve => { finish = resolve; }));
    view.rerender(<ChatPlanning {...props} chatId={8}/>);
    expect(screen.queryByText("Improve search")).toBeNull();
    const other = { ...empty(), chat_id: 8 };
    await act(async () => finish(other));
    await screen.findByText(/No plan yet/);
    expect(service.changeChatPlan).not.toHaveBeenCalled();
  });
  it("rejects responses for another chat on both reads and writes", async () => {
    service.chatPlan.mockResolvedValue({ ...plan(), chat_id: 8 });
    const view = render(<ChatPlanning {...props}/>);
    expect((await screen.findByRole("alert")).textContent).toContain("no longer matches this chat");
    expect(screen.queryByText("Improve search")).toBeNull();
    service.chatPlan.mockResolvedValue(empty());
    view.rerender(<ChatPlanning {...props} tick={1}/>);
    await screen.findByLabelText("Plan title");
    service.changeChatPlan.mockResolvedValue({ ...plan(), chat_id: 8 });
    fireEvent.change(screen.getByLabelText("Plan title"), { target: { value: "My plan" } });
    fireEvent.click(screen.getByRole("button", { name: "Create plan" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Plan response no longer matches this chat");
    expect(screen.queryByText("Improve search")).toBeNull();
    expect(props.onChanged).not.toHaveBeenCalled();
  });
  it("does not overwrite an immediately selected status with its mount effect", () => {
    function ChooseImmediately() {
      useLayoutEffect(() => { fireEvent.change(screen.getByLabelText("S1 status"), { target: { value: "blocked" } }); }, []);
      return <SliceStatus slice={plan().bundle!.slices[0]!} revision={4} save={async () => true}/>;
    }
    render(<ChooseImmediately/>);
    expect(screen.getByLabelText("Blocking reason")).toBeTruthy();
  });
  it("keeps the status choice's original revision after a plan refresh", async () => {
    service.chatPlan.mockResolvedValue(plan());
    const view = render(<ChatPlanning {...props}/>);
    fireEvent.change(await screen.findByLabelText("S1 status"), { target: { value: "blocked" } });
    fireEvent.change(await screen.findByLabelText("Blocking reason"), { target: { value: "Original decision" } });
    service.chatPlan.mockResolvedValue({ ...plan(), revision: 8 });
    view.rerender(<ChatPlanning {...props} tick={1}/>);
    await waitFor(() => expect(service.chatPlan).toHaveBeenCalledTimes(2));
    service.changeChatPlan.mockRejectedValue(Error("stale"));
    fireEvent.click(screen.getByRole("button", { name: "Update status" }));
    await waitFor(() => expect(service.changeChatPlan).toHaveBeenCalledWith(7, expect.objectContaining({ action: "set_slice_status", status: "blocked" }), 4));
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
