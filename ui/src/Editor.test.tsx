import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Editor } from "./Editor";
import { hasGrammar, languageName, themeSlot } from "./codemirror";
import { EditorView } from "@codemirror/view";
import { undo } from "@codemirror/commands";
import { setSynth } from "./ui-preferences";

afterEach(() => {
  vi.unstubAllGlobals();
  setSynth(false);
});

type Call = { url: string; method: string; body: Record<string, unknown> | null };
const writes = (calls: Call[]) => calls.filter((call) => call.url === "/editor/save");

function revision(text: string, hash = `hash-${text.length}`) {
  return { hash, bytes: text.length, device: 1, inode: 7, modified_ms: 1 };
}

/**
 * Routes the editor's endpoints over a fake disk, and records every request.
 *
 * `disk` is mutable on purpose: a test changes it mid-flight to be the other writer.
 */
function stub(
  disk: Record<string, string>,
  options: { hits?: unknown[]; searchFails?: string; stale?: boolean } = {},
) {
  const calls: Call[] = [];
  const opened = new Map<string, string>();
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      const body =
        init?.body === undefined ? null : (JSON.parse(String(init.body)) as Record<string, unknown>);
      const path = new URL(`http://x${url}`).searchParams.get("path") ?? "";
      calls.push({ url: url.split("?")[0] as string, method: init?.method ?? "GET", body });
      const ok = (value: unknown) => Promise.resolve({ ok: true, status: 200, json: async () => value });

      if (url.startsWith("/tree")) {
        const here = Object.keys(disk).filter((name) =>
          path === "" ? !name.includes("/") : name.startsWith(`${path}/`),
        );
        const dirs = new Set(
          Object.keys(disk)
            .filter((name) => name.includes("/") && path === "")
            .map((name) => name.split("/")[0] as string),
        );
        return ok([
          ...[...dirs].map((name) => ({ path: name, name, dir: true })),
          ...here.map((name) => ({
            path: name,
            name: name.slice(name.lastIndexOf("/") + 1),
            dir: false,
          })),
        ]);
      }
      if (url.startsWith("/editor/open")) {
        const text = disk[path];
        opened.set(path, text ?? "");
        return ok({
          path,
          text: text ?? "",
          editable: text !== undefined,
          reason: text === undefined ? "not UTF-8 text" : null,
          revision: text === undefined ? null : revision(text),
          eol: "lf",
          line_separator: "\n",
          encoding: "utf-8",
          bytes: (text ?? "").length,
        });
      }
      if (url.startsWith("/editor/baseline")) {
        return ok({ path, text: opened.get(path) ?? null, tracked: true });
      }
      if (url === "/editor/preview") {
        const proposed = String(body?.text ?? "");
        const current = disk[String(body?.path)] ?? "";
        return ok({
          checkout: "/repo",
          path: String(body?.path),
          base: body?.base,
          disk: revision(current),
          stale: options.stale === true,
          proposed: { hash: `proposed-${proposed.length}`, bytes: proposed.length, eol: "lf" },
          change: {
            hunks: [
              {
                before_start: 1,
                before_lines: 1,
                after_start: 1,
                after_lines: 1,
                lines: [{ kind: "added", before: null, after: 1, text: proposed }],
              },
            ],
            added: 1,
            removed: 0,
            coarse: false,
            truncated: false,
          },
          disk_text: { text: current, truncated: false },
          notices: [],
        });
      }
      if (url === "/editor/save") {
        if (options.stale === true) {
          return Promise.resolve({
            ok: false,
            status: 409,
            json: async () => ({
              reason: "changed_on_disk",
              message: "Changed on disk since you opened it - reload or compare",
              disk: revision(disk[String(body?.path)] ?? ""),
              disk_text: { text: disk[String(body?.path)] ?? "", truncated: false },
              preserved: [],
            }),
          });
        }
        const text = String(body?.text ?? "");
        disk[String(body?.path)] = text;
        return ok({
          path: String(body?.path),
          revision: revision(text, "hash-after"),
          replaced: body?.base,
          // The displaced version is kept, never deleted; the window has to be able to
          // say where it went.
          preserved: {
            name: "1760000000000-9-0-notes.md",
            path: "/repo/.git/ai-team-editor-recovery/1760000000000-9-0-notes.md",
            bytes: 5, hash: "c".repeat(64), foreign: false, in_checkout: false,
          },
          preserve_note: null,
          notified_chats: 1,
        });
      }
      if (url === "/editor/kept/discard") return ok({ discarded: true });
      if (url.startsWith("/editor/kept")) return ok([]);
      if (url.startsWith("/search")) {
        if (options.searchFails !== undefined) {
          return Promise.resolve({
            ok: false,
            status: 400,
            json: async () => ({ error: options.searchFails }),
          });
        }
        return ok(options.hits ?? []);
      }
      if (url.startsWith("/scm")) {
        return ok({ branch: "main", branches: ["main"], unstaged: [], staged: [], untracked: [] });
      }
      if (url.startsWith("/map")) {
        return ok({
          root: "/repo",
          nodes: Object.keys(disk).map((name) => ({
            path: name,
            name,
            dir: false,
            depth: 1,
            weight: 1,
            owner: null,
          })),
          edges: [],
          zones: [],
          unowned: 0,
          files: Object.keys(disk).length,
          truncated: false,
        });
      }
      return ok([]);
    }),
  );
  return calls;
}

/**
 * What the editor is showing.
 *
 * Read off `.cm-content` rather than with `getByText`: CodeMirror splits a highlighted
 * line across spans, so no single element holds the whole line.
 */
async function shown(container: HTMLElement): Promise<string> {
  await waitFor(() => expect(container.querySelector(".cm-content")).not.toBeNull());
  return container.querySelector(".cm-content")?.textContent ?? "";
}

async function type(container: HTMLElement, user: ReturnType<typeof userEvent.setup>, text: string) {
  const line = container.querySelector(".cm-content");
  expect(line).not.toBeNull();
  await user.click(line as Element);
  // jsdom gives a click no coordinates, so CodeMirror leaves the caret at the top of
  // the document. End puts it where a person who clicked the line would have it.
  await user.keyboard("{End}");
  await user.keyboard(text);
}

it("opens a file from the tree and shows its text", async () => {
  const user = userEvent.setup();
  stub({ "README.md": "# hello" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("README.md"));
  expect(await shown(container)).toBe("# hello");
  EditorView.findFromDOM(container.querySelector(".cm-editor")!)?.dispatch({ effects: themeSlot.reconfigure([]) });
  expect(screen.getByRole("textbox", { name: "Source README.md" })).toBe(container.querySelector(".cm-content"));
});

it("retains the actual CodeMirror owner and undo across skins", async () => {
  stub({ "README.md": "Original" });
  const { container } = render(<Editor project="widget" node={null}/>);
  fireEvent.click(await screen.findByText("README.md")); await shown(container);
  const content = screen.getByRole("textbox", { name: "Source README.md" }), view = EditorView.findFromDOM(content)!;
  act(() => view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "Changed" } }));
  act(() => setSynth(true));
  await waitFor(() => expect(container.querySelector(".synth-editor")).not.toBeNull());
  expect(screen.getByRole("textbox", { name: "Source README.md" })).toBe(content);
  act(() => setSynth(false));
  expect(screen.getByRole("textbox", { name: "Source README.md" })).toBe(content);
  act(() => { expect(undo(view)).toBe(true); }); expect(view.state.doc.toString()).toBe("Original");
});
it("a file's name lines up with a folder's, past the same caret slot", async () => {
  // A file's caret slot held one space, which collapses to nothing, so every file name
  // sat a caret's width left of the folder names above it. The slot's width is the
  // stylesheet's (layout.test.ts); here, that every row has one.
  stub({ "src/lib.rs": "fn a() {}", "README.md": "# hello" });
  render(<Editor project="widget" node={null} />);

  for (const name of ["src", "README.md"]) {
    const row = (await screen.findByText(name)).closest("button");
    expect(row?.firstElementChild?.classList.contains("tree__caret")).toBe(true);
  }
});

it("draws the tab strip only once a file is open", async () => {
  // Empty, it was a bare rule across the top of the pane with nothing above it.
  const user = userEvent.setup();
  stub({ "README.md": "# hello" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("README.md"));
  // Tree rows and tabs are both buttons named for the file; the tab is in the strip.
  await waitFor(() => expect(container.querySelector(".editor__tabs")).not.toBeNull());
  await user.click(screen.getByLabelText("close README.md"));
  expect(container.querySelector(".editor__tabs")).toBeNull();
});

it("closing the file in view shows a neighbour, not an empty pane beside open tabs", async () => {
  const user = userEvent.setup();
  stub({ "a.md": "first", "b.md": "second" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("a.md"));
  await waitFor(async () => expect(await shown(container)).toBe("first"));
  await user.click(screen.getAllByText("b.md")[0]!);
  await waitFor(async () => expect(await shown(container)).toBe("second"));

  await user.click(screen.getByLabelText("close b.md"));
  await waitFor(async () => expect(await shown(container)).toBe("first"));
  expect(screen.queryByText("Open a file from the tree, or search for one.")).toBeNull();
});

it("switching tabs keeps the unsaved buffer and its undo history", async () => {
  // Two bugs in one guard: re-reading the file on a tab click threw the human's work
  // away, and rebuilding the CodeMirror state threw away the history that could get it
  // back. The second is the quieter one - the text is right, and undo is empty.
  const user = userEvent.setup();
  const calls = stub({ "a.md": "first", "b.md": "second" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("a.md"));
  await waitFor(async () => expect(await shown(container)).toBe("first"));
  await type(container, user, " edited");
  await waitFor(async () => expect(await shown(container)).toBe("first edited"));

  await user.click(screen.getAllByText("b.md")[0]!);
  await waitFor(async () => expect(await shown(container)).toBe("second"));
  const before = calls.filter((call) => call.url === "/editor/open").length;

  // The tab, not the tree row - both say "a.md".
  await user.click(container.querySelectorAll(".editor__tab")[0] as Element);
  await waitFor(async () => expect(await shown(container)).toBe("first edited"));
  expect(calls.filter((call) => call.url === "/editor/open").length).toBe(before);

  await type(container, user, "{Control>}z{/Control}");
  await waitFor(async () => expect(await shown(container)).toBe("first"));
});

it("Save opens a preview of exactly what will be written and writes nothing yet", async () => {
  const user = userEvent.setup();
  const calls = stub({ "a.md": "first" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("a.md"));
  await shown(container);
  await type(container, user, " and more");
  await waitFor(() => expect(container.querySelector(".editor__dirty")).not.toBeNull());

  await user.click(screen.getByText("Save"));
  const dialog = await screen.findByRole("dialog", { name: "Confirm save" });
  expect(dialog.textContent).toContain("/repo");
  expect(writes(calls)).toHaveLength(0);

  await user.click(screen.getByText("Write these bytes"));
  await waitFor(() => expect(writes(calls)).toHaveLength(1));
  expect(writes(calls)[0]?.body).toMatchObject({ path: "a.md", proposed_hash: "proposed-14" });
  expect((writes(calls)[0]?.body as { text: string }).text).toContain("and more");
  await waitFor(() => expect(container.querySelector(".editor__dirty")).toBeNull());
});

it("a refused save keeps the buffer and offers the version on disk", async () => {
  const user = userEvent.setup();
  stub({ "a.md": "first" }, { stale: true });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("a.md"));
  await shown(container);
  await type(container, user, " mine");
  await user.click(screen.getByText("Save"));
  await user.click(screen.getByText("Write these bytes"));

  expect(
    await screen.findByText("Changed on disk since you opened it - reload or compare"),
  ).toBeDefined();
  // The operator's work is still in the buffer: a refusal that lost it would be worse
  // than the overwrite it prevented.
  expect(await shown(container)).toBe("first mine");
  expect(container.querySelector(".editor__dirty")).not.toBeNull();
});

it("reverting is confirmed first and never touches the file", async () => {
  const user = userEvent.setup();
  const calls = stub({ "a.md": "first" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("a.md"));
  await shown(container);
  await type(container, user, " unwanted");
  await user.click(screen.getByText("Revert"));
  expect(await shown(container)).toBe("first unwanted");

  await user.click(screen.getByText("Discard my changes"));
  await waitFor(async () => expect(await shown(container)).toBe("first"));
  expect(writes(calls)).toHaveLength(0);
});

it("keeps hidden buffers but does not intercept another pane's Save shortcut", async () => {
  const user = userEvent.setup();
  const calls = stub({ "a.md": "first" });
  const view = render(<Editor project="widget" workspace="/widget/task" node={null} />);
  await user.click(await screen.findByText("a.md"));
  await shown(view.container);
  await type(view.container, user, "!");

  view.rerender(<Editor project="widget" workspace="/widget/task" node={null} visible={false} />);
  fireEvent.keyDown(window, { key: "s", metaKey: true });
  expect(screen.queryByRole("dialog", { name: "Confirm save" })).toBeNull();

  view.rerender(<Editor project="widget" workspace="/widget/task" node={null} visible />);
  fireEvent.keyDown(window, { key: "s", metaKey: true });
  await screen.findByRole("dialog", { name: "Confirm save" });
  expect(writes(calls)).toHaveLength(0);
});

it("refuses to open a file that is not text", async () => {
  // A lossy read followed by a save would replace every undecodable byte and quietly
  // corrupt the file.
  const user = userEvent.setup();
  stub({ "logo.png": undefined as unknown as string });
  render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("logo.png"));
  expect(await screen.findByText(/not opening it for editing/)).toBeDefined();
});

it("search opens what it finds", async () => {
  const user = userEvent.setup();
  stub(
    { "src/lib.rs": "fn a() {}" },
    {
      hits: [
        {
          path: "src/lib.rs",
          language: "rust",
          summary: "the library",
          symbol: null,
          score: 0.4,
          start_line: 1,
          end_line: 9,
          snippet: "fn a() {}",
        },
      ],
    },
  );
  const { container } = render(<Editor project="widget" node={null} />);

  await user.type(await screen.findByLabelText("search"), "where is a{Enter}");
  await user.click(await screen.findByText("the library"));
  expect(await shown(container)).toBe("fn a() {}");
});

it("a repo with no index is told how to make one, not handed a worse search", async () => {
  // ai-team having its own substring scan is how the good search stops being used.
  const user = userEvent.setup();
  stub({}, { searchFails: "this checkout has no file-sql index - run `file-sql index` in it" });
  render(<Editor project="widget" node={null} />);

  await user.type(await screen.findByLabelText("search"), "anything{Enter}");
  expect(await screen.findByText(/run `file-sql index`/)).toBeDefined();
});

it("asks for a project before reading any file", async () => {
  const calls = stub({});
  render(<Editor project={null} node={null} />);
  expect(screen.getByText(/Pick a project/)).toBeDefined();
  expect(calls).toHaveLength(0);
});

it("knows a grammar by extension, and does not invent one", async () => {
  // A file is named before it is parsed, and wrong highlighting reads as a syntax error
  // that is not there. PHP, YAML and Dockerfile are Q65's additions.
  for (const path of [
    "src/lib.rs",
    "src/App.tsx",
    "package.json",
    "README.md",
    "web/index.php",
    "ci/config.yaml",
    "Dockerfile",
    "deploy/Dockerfile.ci",
  ]) {
    expect(hasGrammar(path), path).toBe(true);
  }
  for (const path of ["Makefile", "data.parquet"]) expect(hasGrammar(path), path).toBe(false);
  expect(languageName("web/index.php")).toBe("PHP");
  expect(languageName("ci/config.yml")).toBe("YAML");
  expect(languageName("Dockerfile")).toBe("Dockerfile");
  expect(languageName("data.parquet")).toBeNull();
});
