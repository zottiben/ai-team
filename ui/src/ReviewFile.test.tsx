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
