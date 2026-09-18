import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Editor } from "./Editor";
import { languageFor } from "./editor";

afterEach(() => {
  vi.unstubAllGlobals();
});

type Call = { url: string; method: string; body: unknown };

/** Routes the editor's four endpoints and records every write. */
function stub(files: Record<string, string>, hits: unknown[] = [], searchFails?: string) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      calls.push({
        url,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });

      if (url.startsWith("/tree")) {
        const path = new URL(`http://x${url}`).searchParams.get("path") ?? "";
        const here = Object.keys(files).filter((name) =>
          path === "" ? !name.includes("/") : name.startsWith(`${path}/`),
        );
        const dirs = new Set(
          Object.keys(files)
            .filter((name) => name.includes("/") && path === "")
            .map((name) => name.split("/")[0] as string),
        );
        return Promise.resolve({
          ok: true,
          json: async () => [
            ...[...dirs].map((name) => ({ path: name, name, dir: true })),
            ...here.map((name) => ({
              path: name,
              name: name.slice(name.lastIndexOf("/") + 1),
              dir: false,
            })),
          ],
        });
      }
      if (url.startsWith("/file") && init?.method === undefined) {
        const path = new URL(`http://x${url}`).searchParams.get("path") ?? "";
        const text = files[path];
        return Promise.resolve({
          ok: true,
          json: async () => ({ path, text: text ?? "", editable: text !== undefined }),
        });
      }
      if (url === "/file") {
        return Promise.resolve({ ok: true, json: async () => ({ saved: "ok" }) });
      }
      if (url.startsWith("/search")) {
        if (searchFails !== undefined) {
          return Promise.resolve({
            ok: false,
            status: 400,
            json: async () => ({ error: searchFails }),
          });
        }
        return Promise.resolve({ ok: true, json: async () => hits });
      }
      return Promise.resolve({ ok: true, json: async () => [] });
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

it("opens a file from the tree and shows its text", async () => {
  const user = userEvent.setup();
  stub({ "README.md": "# hello" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("README.md"));
  expect(await shown(container)).toBe("# hello");
});

it("switching tabs does not discard unsaved edits by re-reading", async () => {
  // The bug worth guarding: an open buffer is the human's work, and re-fetching it from
  // disk on every tab click silently throws it away.
  const user = userEvent.setup();
  const calls = stub({ "a.md": "first", "b.md": "second" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("a.md"));
  await waitFor(async () => expect(await shown(container)).toBe("first"));
  await user.click(screen.getByText("b.md"));
  await waitFor(async () => expect(await shown(container)).toBe("second"));

  const before = calls.filter((call) => call.url.includes("path=a.md")).length;
  // The tab, not the tree row - both say "a.md".
  const tab = container.querySelectorAll(".editor__tab")[0];
  await user.click(tab as Element);

  await waitFor(async () => expect(await shown(container)).toBe("first"));
  expect(calls.filter((call) => call.url.includes("path=a.md")).length).toBe(before);
});

it("saving sends what is in the buffer and clears the dirty mark", async () => {
  const user = userEvent.setup();
  const calls = stub({ "a.md": "first" });
  const { container } = render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("a.md"));
  await waitFor(async () => expect(await shown(container)).toBe("first"));

  // CodeMirror owns the document, so type into it rather than setting state directly.
  const line = container.querySelector(".cm-content");
  expect(line).not.toBeNull();
  await user.click(line as Element);
  await user.keyboard(" and more");

  await waitFor(() => expect(container.querySelector(".editor__dirty")).not.toBeNull());
  await user.click(screen.getByText("Save"));

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const write = calls.find((call) => call.method === "POST");
  expect(write?.url).toBe("/file");
  expect((write?.body as { text: string }).text).toContain("and more");
  await waitFor(() => expect(container.querySelector(".editor__dirty")).toBeNull());
});

it("refuses to open a file that is not text", async () => {
  // A lossy read followed by a save would replace every undecodable byte and quietly
  // corrupt the file.
  const user = userEvent.setup();
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const url = String(input).replace(/^\/api/, "");
      if (url.startsWith("/tree")) {
        return Promise.resolve({
          ok: true,
          json: async () => [{ path: "logo.png", name: "logo.png", dir: false }],
        });
      }
      return Promise.resolve({
        ok: true,
        json: async () => ({ path: "logo.png", text: "", editable: false }),
      });
    }),
  );
  render(<Editor project="widget" node={null} />);

  await user.click(await screen.findByText("logo.png"));
  expect(await screen.findByText(/is not text/)).toBeDefined();
});

it("search opens what it finds", async () => {
  const user = userEvent.setup();
  stub({ "src/lib.rs": "fn a() {}" }, [
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
  ]);
  const { container } = render(<Editor project="widget" node={null} />);

  await user.type(await screen.findByLabelText("search"), "where is a{Enter}");
  await user.click(await screen.findByText("the library"));
  expect(await shown(container)).toBe("fn a() {}");
});

it("a repo with no index is told how to make one, not handed a worse search", async () => {
  // ai-team having its own substring scan is how the good search stops being used.
  const user = userEvent.setup();
  stub({}, [], "this checkout has no file-sql index - run `file-sql index` in it");
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

it("highlights by extension, and not at all when it does not know one", () => {
  // A file is named before it is parsed, and wrong highlighting reads as a syntax error
  // that is not there.
  expect(languageFor("src/lib.rs")).toHaveLength(1);
  expect(languageFor("src/App.tsx")).toHaveLength(1);
  expect(languageFor("package.json")).toHaveLength(1);
  expect(languageFor("README.md")).toHaveLength(1);
  expect(languageFor("Makefile")).toHaveLength(0);
  expect(languageFor("data.parquet")).toHaveLength(0);
});
