import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Source } from "./Source";

afterEach(() => {
  vi.unstubAllGlobals();
});

function hunk(header: string, text: string) {
  return {
    header,
    old_start: 1,
    new_start: 1,
    lines: [{ kind: "added", old: null, new: 1, text }],
  };
}

function file(path: string, hunks: unknown[]) {
  return {
    path,
    old_path: null,
    status: "modified",
    binary: false,
    hunks,
    additions: hunks.length,
    deletions: 0,
  };
}

function stub(scm: Record<string, unknown>) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      calls.push({
        url,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      if (url.startsWith("/scm?")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            branch: "ai-team/s1",
            branches: ["main", "ai-team/s1"],
            unstaged: [],
            staged: [],
            untracked: [],
            ...scm,
          }),
        });
      }
      return Promise.resolve({ ok: true, json: async () => ({ sha: "a".repeat(40) }) });
    }),
  );
  return calls;
}

it("stages one hunk by its index, not the whole file", async () => {
  // The whole point of per-hunk staging: two changes in one file, and only one of them
  // is ready to go.
  const user = userEvent.setup();
  const calls = stub({
    unstaged: [file("src/lib.rs", [hunk("@@ -1 +1 @@", "top"), hunk("@@ -30 +30 @@", "bottom")])],
  });
  render(<Source project="widget" node={null} />);

  await user.click(await screen.findByText("src/lib.rs"));
  const buttons = await screen.findAllByText("Stage hunk");
  await user.click(buttons[1] as Element);

  await waitFor(() => expect(calls.some((call) => call.url === "/scm/stage")).toBe(true));
  expect(calls.find((call) => call.url === "/scm/stage")?.body).toMatchObject({
    path: "src/lib.rs",
    hunk: 1,
  });
});

it("shows staged and unstaged apart, because their sum answers neither question", async () => {
  stub({
    staged: [file("ready.rs", [hunk("@@ -1 +1 @@", "done")])],
    unstaged: [file("later.rs", [hunk("@@ -1 +1 @@", "not yet")])],
  });
  render(<Source project="widget" node={null} />);

  expect(await screen.findByText(/what a commit would record/)).toBeDefined();
  expect(screen.getByText("ready.rs")).toBeDefined();
  expect(screen.getByText("later.rs")).toBeDefined();
});

it("says which branch beside its heading, not floating mid-header", async () => {
  // Heading, branch picker and Push were three children spaced apart, so "on <branch>"
  // sat alone in the middle of the header.
  stub({});
  render(<Source project="widget" node={null} />);

  const branch = (await screen.findByLabelText("branch")).closest("label");
  const heading = screen.getByRole("heading", { name: "Source control" });
  expect(branch?.parentElement).toBe(heading.parentElement);
  expect(heading.parentElement?.classList.contains("main__header")).toBe(false);
});

it("lists untracked files, which have no diff to appear in", async () => {
  // A view built only from `git diff` leaves an agent's new file out of the commit.
  const user = userEvent.setup();
  const calls = stub({ untracked: ["src/new.rs"] });
  render(<Source project="widget" node={null} />);

  await user.click(await screen.findByText("Stage"));
  await waitFor(() => expect(calls.some((call) => call.url === "/scm/stage")).toBe(true));
  expect(calls.find((call) => call.url === "/scm/stage")?.body).toMatchObject({
    path: "src/new.rs",
  });
});

it("will not commit with nothing staged", async () => {
  // Otherwise it is an empty commit with a message that describes work still sitting in
  // the worktree.
  const user = userEvent.setup();
  stub({ unstaged: [file("later.rs", [hunk("@@ -1 +1 @@", "x")])] });
  render(<Source project="widget" node={null} />);

  await user.type(await screen.findByLabelText("commit message"), "feat: something");
  expect(screen.getByText("Commit").closest("button")?.disabled).toBe(true);
});

it("commits a staged change and reports the sha", async () => {
  const user = userEvent.setup();
  const calls = stub({ staged: [file("ready.rs", [hunk("@@ -1 +1 @@", "done")])] });
  render(<Source project="widget" node={null} />);

  await user.type(await screen.findByLabelText("commit message"), "feat: add the thing");
  await user.click(screen.getByText("Commit"));

  await waitFor(() => expect(calls.some((call) => call.url === "/scm/commit")).toBe(true));
  expect(calls.find((call) => call.url === "/scm/commit")?.body).toMatchObject({
    message: "feat: add the thing",
  });
  expect(await screen.findByText(/Committed aaaaaaa/)).toBeDefined();
});

it("switches branch and pushes", async () => {
  const user = userEvent.setup();
  const calls = stub({});
  render(<Source project="widget" node={null} />);

  await user.selectOptions(await screen.findByLabelText("branch"), "main");
  await waitFor(() => expect(calls.some((call) => call.url === "/scm/branch")).toBe(true));

  await user.click(screen.getByText("Push"));
  await waitFor(() => expect(calls.some((call) => call.url === "/scm/push")).toBe(true));
});

it("says plainly when nothing has changed", async () => {
  stub({});
  render(<Source project="widget" node={null} />);
  expect(await screen.findByText(/Nothing has changed/)).toBeDefined();
});
