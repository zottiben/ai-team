import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type { RunEvent } from "../../../../api";
import Tracker, { type EventDisclosure } from "./Tracker";

const eventKey = (event: RunEvent) => event.id;
function message(id: number, text = "Keep this message"): RunEvent {
  return { id, kind: "note", actor: "human", message: text, summary: text, thinking: [], node_run_id: 1, session_id: "session", at: "coarse" };
}
function tool(id: number, kind: "tool_call" | "tool_result", call: string, failed = false): RunEvent {
  return { ...message(id), kind, message: null, actor: "agent", summary: kind === "tool_call" ? "bash" : "bash returned",
    tool: { call_id: call, parent_call_id: null, name: "bash", target: kind === "tool_call" ? { kind: "command", text: `echo ${call}`, truncated: false } : null,
      outcome: kind === "tool_result" ? failed ? "failed" : "returned" : null, duration_ms: kind === "tool_result" ? 125.5 : null,
      output: kind === "tool_result" ? { text: `Output ${call}`, truncated: true } : null } };
}
const renderEvent = (event: RunEvent, disclosure: EventDisclosure) => <>
  {event.thinking.length > 0 && <details open={disclosure.open} onToggle={e => disclosure.onToggle(e.currentTarget.open)}><summary>Thinking summary</summary><p>{event.thinking.join(" ")}</p></details>}
  {event.message !== null && <article aria-label="You">{event.message}</article>}
</>;
const props = { synth: true, visible: true, pattern: false, eventKey, renderEvent, empty: <p>Empty fixture</p>, tail: <p>Retained receipts</p> };
beforeEach(() => { document.documentElement.dataset.synth = "true"; });

describe("shared tracker rendering", () => {
  it("pairs parallel calls, exposes bounded output and never filters away failures or messages", async () => {
    const events = [message(1), tool(2, "tool_call", "a"), tool(3, "tool_call", "b"), tool(4, "tool_result", "b", true), tool(5, "tool_result", "a")];
    const view = render(<Tracker {...props} events={events}/>);
    await screen.findByRole("group", { name: "Activity lanes" });
    expect(view.container.querySelectorAll(".synth-tracker-tool")).toHaveLength(2);
    expect(screen.getByText(/2 observed overlapping calls/)).toBeTruthy();
    expect(screen.getAllByText("125.5 ms runtime")).toHaveLength(2);
    fireEvent.click(within(screen.getByRole("group", { name: "Activity lanes" })).getByRole("button", { name: "Bash" }));
    expect(screen.queryByText("echo a")).toBeNull();
    expect(screen.getByText("echo b")).toBeTruthy();
    expect(screen.getByRole("article", { name: "You" })).toBeTruthy();
    expect(screen.getByText("Retained receipts")).toBeTruthy();
    const output = screen.getByText("Completed output · tail truncated").closest("details")!;
    output.open = true; fireEvent(output, new Event("toggle"));
    await waitFor(() => expect(output.open).toBe(true));
  });

  it("shows recorded native edit counts without inventing counts for writes", async () => {
    const start = tool(1, "tool_call", "edit"), end = tool(2, "tool_result", "edit");
    if (!start.tool || !end.tool) throw new Error("Missing fixture metadata");
    start.tool.name = end.tool.name = "edit";
    end.tool.edits = { added: 3, removed: 2, first_changed_line: 42, source: "pi_edit_diff" };
    const view = render(<Tracker {...props} events={[start, end]}/>);
    expect(screen.getByText("+3 −2 reported lines · first change 42")).toBeTruthy();
    view.rerender(<Tracker {...props} events={[{ ...start, tool: { ...start.tool, name: "write" } }]}/>);
    expect(screen.getByText("Edit counts unavailable")).toBeTruthy();
    expect(screen.queryByText(/reported lines/)).toBeNull();
  });

  it("keeps message DOM and expanded summaries across both skins", async () => {
    const thought = { ...message(2), kind: "cost", message: null, thinking: ["Inspecting safely"] };
    const events = [message(1), thought];
    const view = render(<Tracker {...props} synth={false} events={events}/>);
    const node = screen.getByRole("article", { name: "You" });
    const old = screen.getByText("Thinking summary").closest("details")!;
    old.open = true; fireEvent(old, new Event("toggle"));
    await waitFor(() => expect(old.open).toBe(true));
    view.rerender(<Tracker {...props} events={events}/>);
    await screen.findByRole("group", { name: "Activity lanes" });
    expect(screen.getByRole("article", { name: "You" })).toBe(node);
    expect(screen.getByText("Inspecting safely")).toBeTruthy();
    expect(screen.getByText(/Thinking summary/).closest("details")?.open).toBe(true);
    view.rerender(<Tracker {...props} synth={false} events={events}/>);
    expect(screen.getByRole("article", { name: "You" })).toBe(node);
    expect(screen.getByText("Thinking summary").closest("details")?.open).toBe(true);
  });

  it("keeps summaries arriving into an expanded group open when returning to the legacy skin", async () => {
    const first = { ...message(1), actor: "agent", kind: "cost", message: null, thinking: ["First summary"] };
    const view = render(<Tracker {...props} events={[first]}/>);
    const details = screen.getByText(/Thinking summary/).closest("details")!;
    details.open = true; fireEvent(details, new Event("toggle"));
    await screen.findByText("First summary");
    const events = [first, { ...first, id: 2, thinking: ["New summary"] }];
    view.rerender(<Tracker {...props} events={events}/>);
    await screen.findByText("New summary");
    view.rerender(<Tracker {...props} synth={false} events={events}/>);
    const summaries = [...view.container.querySelectorAll("details")];
    expect(summaries).toHaveLength(2);
    expect(summaries.every(summary => summary.open)).toBe(true);
  });

  it("follows within 80px only and announces genuinely new rows while held", async () => {
    const events = [message(1)];
    const view = render(<Tracker {...props} events={events}/>);
    await screen.findByRole("group", { name: "Activity lanes" });
    const scroll = view.container.querySelector<HTMLDivElement>(".chat-transcript")!;
    Object.defineProperties(scroll, { scrollHeight: { configurable: true, value: 1000 }, clientHeight: { configurable: true, value: 200 } });
    scroll.scrollTop = 719; fireEvent.scroll(scroll);
    expect(screen.getByRole("button", { name: "↓ 0 new" })).toBeTruthy();
    view.rerender(<Tracker {...props} events={[...events, message(2, "New evidence")]}/>);
    await screen.findByRole("button", { name: "↓ 1 new" });
    expect(scroll.scrollTop).toBe(719);
    expect(screen.getByRole("status").textContent).toBe("1 new activity rows");
    scroll.scrollTop = 720; fireEvent.scroll(scroll);
    await waitFor(() => expect(screen.queryByRole("button", { name: /↓/ })).toBeNull());
    expect(scroll.scrollTop).toBe(1000);
  });

  it("sequence selection reveals a filtered row and focuses it without sending anything", async () => {
    const view = render(<Tracker {...props} pattern events={[tool(1, "tool_call", "exact"), tool(2, "tool_result", "exact")]}/>);
    const lanes = await screen.findByRole("group", { name: "Activity lanes" });
    fireEvent.click(within(lanes).getByRole("button", { name: "Bash" }));
    expect(screen.queryByText("echo exact")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Bash · echo exact · event 1/ }));
    await waitFor(() => expect(document.activeElement).toBe(view.container.querySelector('[data-row-id="1"]')));
    expect(screen.getByText("echo exact")).toBeTruthy();
    const scroll = view.container.querySelector(".chat-transcript")!;
    fireEvent.scroll(scroll);
    expect(screen.getByRole("button", { name: "↓ 0 new" })).toBeTruthy(); // Programmatic near-end jumps must not re-enable follow.
    fireEvent.wheel(scroll); fireEvent.scroll(scroll);
    expect(screen.queryByRole("button", { name: "↓ 0 new" })).toBeNull();
  });

  it("counts arrivals inside read groups without counting later results, and retains only failed reads when filtered", async () => {
    const read = (event: RunEvent): RunEvent => ({ ...event, tool: event.tool ? { ...event.tool, name: "read" } : null });
    const events = [tool(1, "tool_call", "a"), tool(2, "tool_result", "a"), tool(3, "tool_call", "b"), tool(4, "tool_result", "b", true)].map(read);
    const view = render(<Tracker {...props} events={events}/>);
    const lanes = await screen.findByRole("group", { name: "Activity lanes" });
    const scroll = view.container.querySelector<HTMLDivElement>(".chat-transcript")!;
    Object.defineProperties(scroll, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 } });
    scroll.scrollTop = 0; fireEvent.scroll(scroll);
    events.push(read(tool(5, "tool_call", "c")));
    view.rerender(<Tracker {...props} events={[...events]}/>);
    await screen.findByRole("button", { name: "↓ 1 new" });
    events.push(read(tool(6, "tool_result", "c")));
    view.rerender(<Tracker {...props} events={[...events]}/>);
    expect(screen.getByRole("button", { name: "↓ 1 new" })).toBeTruthy();
    fireEvent.click(within(lanes).getByRole("button", { name: "Read" }));
    expect(screen.queryByText("echo a")).toBeNull();
    expect(screen.queryByText("echo c")).toBeNull();
    expect(screen.getByText("echo b")).toBeTruthy();
  });

  it("does not report existing results as new when switching skins while held", async () => {
    const events = [tool(1, "tool_call", "a"), tool(2, "tool_result", "a")];
    const view = render(<Tracker {...props} events={events}/>);
    await screen.findByRole("group", { name: "Activity lanes" });
    const scroll = view.container.querySelector<HTMLDivElement>(".chat-transcript")!;
    Object.defineProperties(scroll, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 } });
    scroll.scrollTop = 0; fireEvent.scroll(scroll);
    view.rerender(<Tracker {...props} synth={false} events={events}/>);
    view.rerender(<Tracker {...props} events={events}/>);
    await screen.findByRole("button", { name: "↓ 0 new" });
  });
});
