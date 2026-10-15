import { readFileSync } from "node:fs";
import { join } from "node:path";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Editor } from "../../../Editor";
import { setSynth } from "../../../ui-preferences";

afterEach(() => {
  vi.unstubAllGlobals();
  setSynth(false);
});

const REVISION = { hash: "a".repeat(64), bytes: 12, device: 1, inode: 2, modified_ms: 1 };

type Kept = {
  name: string; path: string; bytes: number; hash: string;
  foreign: boolean; in_checkout: boolean;
};

type Options = { stale?: boolean; working?: boolean; kept?: Kept | null };

function stub(disk: Record<string, string>, options: Options = {}) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      const body = init?.body === undefined ? null : (JSON.parse(String(init.body)) as Record<string, unknown>);
      const path = new URL(`http://x${url}`).searchParams.get("path") ?? "";
      calls.push(url.split("?")[0] as string);
      const ok = (value: unknown) => Promise.resolve({ ok: true, status: 200, json: async () => value });

      if (url.startsWith("/tree")) {
        return ok(Object.keys(disk).map(name => ({ path: name, name, dir: false })));
      }
      if (url.startsWith("/editor/open")) {
        return ok({
          path, text: disk[path] ?? "", editable: true, reason: null,
          revision: { ...REVISION, bytes: (disk[path] ?? "").length },
          eol: "crlf", line_separator: "\r\n", encoding: "utf-8", bytes: (disk[path] ?? "").length,
        });
      }
      if (url.startsWith("/editor/baseline")) return ok({ path, text: "first\n", tracked: true });
      if (url === "/editor/preview") {
        const proposed = String(body?.text ?? "");
        return ok({
          checkout: "/repo/checkout", path: String(body?.path), base: body?.base,
          disk: REVISION, stale: options.stale === true,
          proposed: { hash: "b".repeat(64), bytes: proposed.length, eol: "crlf" },
          change: {
            hunks: [{
              before_start: 1, before_lines: 1, after_start: 1, after_lines: 1,
              lines: [
                { kind: "removed", before: 1, after: null, text: "first" },
                { kind: "added", before: null, after: 1, text: proposed },
              ],
            }],
            added: 1, removed: 1, coarse: false, truncated: false,
          },
          disk_text: { text: "first\n", truncated: false },
          notices: ["No newline at end of file."],
        });
      }
      if (url === "/editor/save") {
        if (options.stale === true) {
          return Promise.resolve({
            ok: false, status: 409,
            json: async () => ({
              reason: "changed_on_disk",
              message: "Changed on disk since you opened it - reload or compare",
              disk: REVISION,
              disk_text: { text: "somebody else\n", truncated: false },
              preserved: [],
            }),
          });
        }
        return ok({
          path: String(body?.path), revision: REVISION, replaced: body?.base,
          preserved: options.kept ?? null, preserve_note: null, notified_chats: 2,
        });
      }
      if (url === "/editor/kept/discard") return ok({ discarded: true });
      if (url.startsWith("/editor/kept")) return ok([]);
      if (url.startsWith("/scm")) {
        return ok({
          branch: "synth-ui", branches: ["synth-ui"], staged: [], unstaged: [],
          untracked: ["notes.md"],
        });
      }
      if (url.startsWith("/map")) {
        return ok({
          root: "/repo/checkout",
          nodes: Object.keys(disk).map(name => ({ path: name, name, dir: false, depth: 1, weight: 1, owner: null })),
          edges: [], zones: [], unowned: 0, files: 1, truncated: false,
        });
      }
      if (url.includes("/overview")) {
        return ok({
          chat_id: 4, project: "widget", workspace: "/repo/checkout", mode: "single",
          seats: [{
            role: "assistant", node_id: 1, run_id: 1, provider: "local", model: "fixture",
            status: "running", attempt: 1, slice_key: null, worktree: "/repo/checkout",
            started_at: new Date(Date.now() - 12_000).toISOString(), ended_at: null,
            blocked_reason: null, live: options.working === true, supervised: options.working === true,
            activity: options.working === true
              ? { kind: "tool_call", summary: "reading", detail: "notes.md", file: "notes.md", at: new Date().toISOString() }
              : null,
            said: null, runs: 1, steps: 1,
            usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 },
            context_tokens: null,
            touches: [{ path: "notes.md", tool: "read", reads: 1, writes: 0, at: new Date().toISOString() }],
            outside: 0,
          }],
          totals: { turns: 1, steps: 1, usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 }, files_touched: 1, files_written: 0 },
          nodes_total: 1, nodes_read: 1,
        });
      }
      return ok([]);
    }),
  );
  return calls;
}

async function openNotes(user: ReturnType<typeof userEvent.setup>, container: HTMLElement) {
  await user.click((await screen.findAllByText("notes.md"))[0]!);
  await waitFor(() => expect(container.querySelector(".cm-content")).not.toBeNull());
}

const shown = (container: HTMLElement) => container.querySelector(".cm-content")?.textContent ?? "";

async function type(container: HTMLElement, user: ReturnType<typeof userEvent.setup>, text: string) {
  await user.click(container.querySelector(".cm-content") as Element);
  await user.keyboard("{End}");
  await user.keyboard(text);
}

it("draws the spec's parts: browser, tabs, code, map, monitor and status", async () => {
  setSynth(true);
  const user = userEvent.setup();
  stub({ "notes.md": "first" }, { working: true });
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  expect(screen.getByLabelText("Go to file")).toBeDefined();
  expect(screen.getByRole("tablist", { name: "Open files" })).toBeDefined();
  expect(screen.getByRole("scrollbar", { name: "File map" })).toBeDefined();
  expect(screen.getByLabelText("Agent monitor")).toBeDefined();
  // §11.1's warning survives the reskin.
  expect(screen.getByText(/operator tool, not an OS sandbox/)).toBeDefined();

  const status = screen.getByLabelText("Editor status");
  await waitFor(() => expect(status.textContent).toContain("Ln 1, Col 1"));
  expect(status.textContent).toContain("synth-ui");
  expect(status.textContent).toContain("CRLF");
  expect(status.textContent).toContain("UTF-8");
  expect(status.textContent).toContain("Markdown");
});

it("with no reported position the agent tag sits at the top of the viewport, not on a line", async () => {
  // §11.2's fallback. A caret drawn at a guessed line would be a fabrication of where
  // somebody else is working.
  setSynth(true);
  const user = userEvent.setup();
  stub({ "notes.md": "first" }, { working: true });
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  const tag = await waitFor(() => {
    const found = container.querySelector(".synth-editor__presence");
    expect(found).not.toBeNull();
    return found as HTMLElement;
  });
  expect(tag.dataset.placement).toBe("viewport-top");
  expect(tag.textContent).toContain("assistant");
  expect(tag.textContent).toContain("no line reported");
  expect(container.querySelector(".synth-editor__presence [class*='caret']")).toBeNull();
});

it("says nothing about an agent that is not running here", async () => {
  setSynth(true);
  const user = userEvent.setup();
  stub({ "notes.md": "first" }, { working: false });
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  await waitFor(() => expect(screen.getByLabelText("Editor status").textContent).toContain("No agent running here"));
  expect(container.querySelector(".synth-editor__presence")).toBeNull();
});

it("the preview names the exact checkout, bytes and hash before anything is written", async () => {
  setSynth(true);
  const user = userEvent.setup();
  const calls = stub({ "notes.md": "first" });
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  await type(container, user, "!");
  await user.click(screen.getByText("Save ⌘S"));

  const dialog = await screen.findByRole("dialog", { name: "Save notes.md" });
  expect(dialog.textContent).toContain("/repo/checkout");
  expect(dialog.textContent).toContain("bbbbbbbbbbbb");
  expect(dialog.textContent).toContain("No newline at end of file.");
  expect(calls).not.toContain("/editor/save");

  await user.click(screen.getByText("Write these bytes"));
  await waitFor(() => expect(calls).toContain("/editor/save"));
  // The agent is told *after* a successful write, and the window says how many chats.
  expect(await screen.findByText(/2 chats will be told at the start of the next turn/)).toBeDefined();
});

it("a stale preview cannot be confirmed at all", async () => {
  setSynth(true);
  const user = userEvent.setup();
  const calls = stub({ "notes.md": "first" }, { stale: true });
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  await type(container, user, "!");
  await user.click(screen.getByText("Save ⌘S"));

  expect(await screen.findByText("Changed on disk since you opened it - reload or compare")).toBeDefined();
  expect(screen.getByText("Write these bytes").closest("button")?.disabled).toBe(true);
  expect(calls).not.toContain("/editor/save");
});

it("a refusal after confirming keeps the buffer and offers both versions", async () => {
  setSynth(true);
  const user = userEvent.setup();
  // Not stale at preview time, refused at write time: the race this exists for.
  const options: Options = {};
  const calls = stub({ "notes.md": "first" }, options);
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  await type(container, user, " mine");
  await user.click(screen.getByText("Save ⌘S"));
  options.stale = true;
  await user.click(screen.getByText("Write these bytes"));

  const refusal = await screen.findByRole("alertdialog", { name: "Save refused" });
  expect(refusal.textContent).toContain("Changed on disk since you opened it - reload or compare");
  expect(refusal.textContent).toContain("somebody else");
  expect(shown(container)).toBe("first mine");
  expect(calls.filter(call => call === "/editor/save")).toHaveLength(1);
});

it("a save says where the version it replaced is kept, and lets the operator drop it", async () => {
  // The backend swaps two files rather than renaming over one, so the previous version
  // is still on disk - a descriptor another program is holding keeps writing into it.
  // A window that did not say so would be hiding a file rather than keeping one, and
  // only a person may decide it can go.
  setSynth(true);
  const user = userEvent.setup();
  const calls = stub(
    { "notes.md": "first" },
    {
      kept: {
        name: "1760000000000-9-0-notes.md",
        path: "/repo/checkout/.git/ai-team-editor-recovery/1760000000000-9-0-notes.md",
        bytes: 5, hash: "c".repeat(64), foreign: false, in_checkout: false,
      },
    },
  );
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  await type(container, user, "!");
  await user.click(screen.getByText("Save ⌘S"));
  await user.click(screen.getByText("Write these bytes"));

  const receipt = await screen.findByRole("status");
  expect(receipt.textContent).toContain("ai-team-editor-recovery/1760000000000-9-0-notes.md");
  expect(calls).not.toContain("/editor/kept/discard");

  await user.click(screen.getByText("Discard kept version"));
  await waitFor(() => expect(calls).toContain("/editor/kept/discard"));
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).not.toContain("ai-team-editor-recovery"),
  );
});

it("switching skin keeps the buffer, its unsaved edit and its undo history", async () => {
  // The flag is a skin, not a new editor. Rebuilding the document on a toggle would
  // throw away work that was never on disk.
  const user = userEvent.setup();
  const calls = stub({ "notes.md": "first" });
  const { container } = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await openNotes(user, container);
  await type(container, user, " edited");
  expect(shown(container)).toBe("first edited");
  const opens = calls.filter(call => call === "/editor/open").length;

  setSynth(true);
  await screen.findByLabelText("Agent monitor");
  expect(shown(container)).toBe("first edited");
  expect(calls.filter(call => call === "/editor/open").length).toBe(opens);

  await type(container, user, "{Control>}z{/Control}");
  await waitFor(() => expect(shown(container)).toBe("first"));

  setSynth(false);
  await waitFor(() => expect(screen.queryByLabelText("Agent monitor")).toBeNull());
  expect(shown(container)).toBe("first");
});

it("stops every loop when the window asks for reduced motion", () => {
  // There is no browser on the machine this is built on, so the rule is read off the
  // stylesheet rather than looked at.
  const css = readFileSync(join(import.meta.dirname, "editor.css"), "utf8");
  const reduced = css.slice(css.indexOf("@media (prefers-reduced-motion: reduce)"));
  expect(reduced).toContain(".synth-editor__presence { transition: none; }");
  expect(reduced).toContain(".cm-cursorLayer { animation: none; }");
});

it("finds a file by name, and ⌘P is the editor's only while the editor is up", async () => {
  // Both halves of §11.2's browser: the filename index the LCD advertises, and the
  // file-sql content search the old editor had, which this must not quietly drop.
  setSynth(true);
  const user = userEvent.setup();
  stub({ "notes.md": "first", "src/deep/thing.rs": "fn a() {}" });
  const view = render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} visible />);

  await screen.findByLabelText("Go to file");
  fireEvent.keyDown(window, { key: "p", metaKey: true });
  await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Go to file")));

  await user.type(screen.getByLabelText("Go to file"), "deep");
  // The tree lists the same path; the match list is the one the finder renders.
  const match = await waitFor(() => {
    const found = view.container.querySelector(".synth-editor__match");
    expect(found?.textContent).toBe("src/deep/thing.rs");
    return found as HTMLElement;
  });
  await user.click(match);
  await waitFor(() => expect(view.container.querySelector(".cm-content")?.textContent).toBe("fn a() {}"));

  // Hidden, the shortcut belongs to whatever else is on screen.
  view.rerender(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} visible={false} />);
  (document.activeElement as HTMLElement | null)?.blur();
  fireEvent.keyDown(window, { key: "p", metaKey: true });
  expect(document.activeElement).not.toBe(screen.getByLabelText("Go to file"));
});

it("keeps the content search the old editor had", async () => {
  setSynth(true);
  const user = userEvent.setup();
  stub({ "notes.md": "first" });
  render(<Editor project="widget" workspace="/repo/checkout" node={null} chat={4} />);

  await user.click(await screen.findByText("Content"));
  const input = screen.getByLabelText("Search file contents");
  await user.type(input, "anything{Enter}");
  expect(await screen.findByText("No hits.")).toBeDefined();
});
