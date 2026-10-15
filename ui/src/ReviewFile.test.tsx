import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { webcrypto } from "node:crypto";
import { FileView } from "./ReviewFile";
import type { FileDiff } from "./api";

const file = (text = "new code"): FileDiff => ({ path: "new.rs", old_path: null, status: "added", binary: false, additions: 1, deletions: 0, hunks: [{ header: "@@ -0,0 +1 @@", old_start: 0, new_start: 1, lines: [{ kind: "added", old: null, new: 1, text }] }] });
beforeEach(() => { localStorage.clear(); vi.stubGlobal("crypto", webcrypto); });
afterEach(() => vi.unstubAllGlobals());
it("expands ordinary files and collapses them without losing an unsent comment", () => {
  const onComment = vi.fn();
  render(<FileView file={file()} reviewKey="chat:1:working" onComment={onComment} />);
  const toggle = screen.getByRole("button", { name: "Collapse new.rs" });
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "comment on new line 1" }));
  fireEvent.change(screen.getByLabelText("comment on new.rs new line 1"), { target: { value: "Keep this draft" } });
  fireEvent.click(toggle);
  expect(screen.queryByRole("textbox")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Expand new.rs" }));
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("Keep this draft");
  expect(onComment).not.toHaveBeenCalled();
});
it("does not silently retarget an unsent comment when the diff changes", () => {
  const before = vi.fn().mockResolvedValue(true), after = vi.fn().mockResolvedValue(true);
  const view = render(<FileView file={file("original line")} reviewKey="chat:1:working" onComment={before} />);
  fireEvent.click(screen.getByRole("button", { name: "comment on new line 1" }));
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "Fix the original value" } });
  view.rerender(<FileView file={file("a completely different line")} reviewKey="chat:1:working" onComment={after} />);
  fireEvent.click(screen.getByRole("button", { name: "Comment" }));
  expect(before).not.toHaveBeenCalled(); expect(after).not.toHaveBeenCalled();
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("Fix the original value");
  expect(screen.getByText(/anchor changed/i)).toBeTruthy();
});

it("retains a removed-line draft and requires explicit reanchoring before a first submission", async () => {
  const before=vi.fn().mockResolvedValue(true), after=vi.fn().mockResolvedValue(true);
  const view=render(<FileView file={file("original")} reviewKey="chat:1" commentScope="epoch:1" onComment={before}/>);
  fireEvent.click(screen.getByRole("button",{name:"comment on new line 1"}));
  fireEvent.change(screen.getByRole("textbox"),{target:{value:"Keep this exact draft"}});
  view.rerender(<FileView file={{...file(),hunks:[]}} reviewKey="chat:1" commentScope="epoch:2" onComment={after}/>);
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("Keep this exact draft");
  fireEvent.click(screen.getByRole("button",{name:"Comment"})); expect(after).not.toHaveBeenCalled();
  view.rerender(<FileView file={file("replacement")} reviewKey="chat:1" commentScope="epoch:2" onComment={after}/>);
  fireEvent.click(screen.getByRole("button",{name:"comment on new line 1"}));
  fireEvent.click(screen.getByRole("button",{name:"Comment"}));
  await waitFor(()=>expect(after).toHaveBeenCalledWith("Keep this exact draft",expect.objectContaining({side:"new",line_start:1})));
  expect(before).not.toHaveBeenCalled();
});
it("keeps an active composer mounted while submission is temporarily unavailable", () => {
  const view=render(<FileView file={file()} onComment={vi.fn()}/>);
  fireEvent.click(screen.getByRole("button",{name:"comment on new line 1"}));
  const input=screen.getByRole("textbox"); fireEvent.change(input,{target:{value:"Wait for the current action"}});
  view.rerender(<FileView file={file()}/>);
  expect(screen.getByRole("textbox")).toBe(input);
  expect((screen.getByRole("button",{name:"Comment"}) as HTMLButtonElement).disabled).toBe(true);
});

it("does not promise idempotent replay for a legacy comment sender",async()=>{
 const before=vi.fn().mockRejectedValue(Error("response unknown")), after=vi.fn();
 const view=render(<FileView file={file()} onComment={before}/>);
 fireEvent.click(screen.getByRole("button",{name:"comment on new line 1"}));
 fireEvent.change(screen.getByRole("textbox"),{target:{value:"Original comment"}});
 fireEvent.click(screen.getByRole("button",{name:"Comment"})); await screen.findByRole("alert");
 view.rerender(<FileView file={file("changed")} onComment={after}/>);
 expect((screen.getByRole("button",{name:"Comment"}) as HTMLButtonElement).disabled).toBe(true);
 expect(screen.queryByText(/Retry uses the original request/)).toBeNull();
});

it("persists viewed progress, scopes it, and clears it when the file changes", async () => {
  const view = render(<FileView file={file()} reviewKey="chat:1:unstaged" />);
  fireEvent.click(screen.getByRole("checkbox", { name: "Viewed new.rs" }));
  expect(screen.getByRole("button", { name: "Expand new.rs" })).toBeTruthy();
  await waitFor(() => expect(localStorage.getItem("ai-team.review-files.v1")).toContain('"viewed":true'));
  expect(localStorage.getItem("ai-team.review-files.v1")).not.toContain("new code");
  expect(localStorage.getItem("ai-team.review-files.v1")).not.toContain("new.rs");
  expect(localStorage.getItem("ai-team.review-files.v1")).not.toContain("chat:1");
  view.unmount();
  const again = render(<FileView file={file()} reviewKey="chat:1:unstaged" />);
  await waitFor(() => expect((screen.getByRole("checkbox", { name: "Viewed new.rs" }) as HTMLInputElement).checked).toBe(true));
  again.rerender(<FileView file={file("changed code")} reviewKey="chat:1:unstaged" />);
  expect((screen.getByRole("checkbox", { name: "Viewed new.rs" }) as HTMLInputElement).checked).toBe(false);
  expect(screen.getByRole("button", { name: "Collapse new.rs" })).toBeTruthy();
  fireEvent.click(screen.getByRole("checkbox", { name: "Viewed new.rs" }));
  again.rerender(<FileView file={file("changed code")} reviewKey="chat:2:unstaged" />);
  expect((screen.getByRole("checkbox", { name: "Viewed new.rs" }) as HTMLInputElement).checked).toBe(false);
});
it("clears Viewed when untracked bytes change without changing displayed lines", () => {
  const view = render(<FileView file={file()} reviewKey="chat:1:untracked" revision="without-final-newline" />);
  fireEvent.click(screen.getByRole("checkbox", { name: "Viewed new.rs" }));
  view.rerender(<FileView file={file()} reviewKey="chat:1:untracked" revision="with-final-newline" />);
  expect((screen.getByRole("checkbox", { name: "Viewed new.rs" }) as HTMLInputElement).checked).toBe(false);
  expect(screen.getByRole("button", { name: "Collapse new.rs" })).toBeTruthy();
});
it("does not reopen a manually collapsed file on identical polling data", () => {
  const view = render(<FileView file={file()} reviewKey="chat:1:staged" />);
  fireEvent.click(screen.getByRole("button", { name: "Collapse new.rs" }));
  view.rerender(<FileView file={file()} reviewKey="chat:1:staged" />);
  expect(screen.getByRole("button", { name: "Expand new.rs" })).toBeTruthy();
});
