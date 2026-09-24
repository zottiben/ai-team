import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Review } from "./Review";

afterEach(() => {
  vi.unstubAllGlobals();
});

const FILE = {
  path: "src/lib.rs",
  old_path: null,
  status: "modified",
  binary: false,
  additions: 2,
  deletions: 1,
  hunks: [
    {
      header: "@@ -10,4 +10,5 @@",
      old_start: 10,
      new_start: 10,
      lines: [
        { kind: "context", old: 10, new: 10, text: "fn one() {}" },
        { kind: "removed", old: 11, new: null, text: "fn two() {}" },
        { kind: "added", old: null, new: 11, text: "fn two(x: i32) {}" },
        { kind: "added", old: null, new: 12, text: "fn three() {}" },
      ],
    },
  ],
};

function detail(over: Record<string, unknown> = {}) {
  return {
    id: 3,
    project_id: 1,
    run_id: 1,
    node_run_id: 5,
    title: "PR1: subtract",
    status: "open",
    branch: "ai-team/s1",
    submitted_at: null,
    files: [FILE],
    comments: [],
    steerable: true,
    ...over,
  };
}

/** Routes the two paths the view uses, and records writes. */
function stub(over: Record<string, unknown> = {}, list: unknown[] = [{ id: 3, title: "PR1: subtract", branch: "ai-team/s1" }]) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      const path = String(url).replace(/^\/api/, "");
      calls.push({
        url: path,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      if (path.startsWith("/reviews?")) {
        return Promise.resolve({ ok: true, json: async () => list });
      }
      if (path === "/reviews/3/submit") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            outcome: "steered",
            node_run_id: 5,
            comments: 2,
            told_orchestrator: true,
          }),
        });
      }
      return Promise.resolve({ ok: true, json: async () => detail(over) });
    }),
  );
  return calls;
}

async function openReview(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByText("PR1: subtract"));
}

it("says before submitting that a finished seat's comments go back into its pull request", async () => {
  const user = userEvent.setup();
  stub({ steerable: false, follows_up: true });
  render(<Review tick={0} />);
  await openReview(user);

  expect(
    (await screen.findByText(/still open where it was built/)).textContent,
  ).toContain("checked again");
});

it("anchors a comment to the side and line the human clicked", async () => {
  // A comment on a removed line and one on an added line at the same number are
  // different comments. Losing the side is how feedback lands on the wrong code.
  const user = userEvent.setup();
  const calls = stub();
  render(<Review tick={0} />);
  await openReview(user);

  await user.click(await screen.findByLabelText("comment on old line 11"));
  await user.type(
    await screen.findByLabelText("comment on src/lib.rs old line 11"),
    "why was this dropped?",
  );
  await user.click(screen.getByText("Comment"));

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const write = calls.find((call) => call.method === "POST");
  expect(write?.url).toBe("/reviews/3/comments");
  expect(write?.body).toMatchObject({
    file_path: "src/lib.rs",
    side: "old",
    line_start: 11,
    line_end: 11,
    body: "why was this dropped?",
  });
});

it("a comment on an added line records the new side", async () => {
  const user = userEvent.setup();
  const calls = stub();
  render(<Review tick={0} />);
  await openReview(user);

  await user.click(await screen.findByLabelText("comment on new line 12"));
  await user.type(await screen.findByLabelText("comment on src/lib.rs new line 12"), "needs a test");
  await user.click(screen.getByText("Comment"));

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  expect(calls.find((call) => call.method === "POST")?.body).toMatchObject({
    side: "new",
    line_start: 12,
  });
});

it("says which of the two things submitting will do, before it is pressed", async () => {
  // One lands in a worktree that still exists; the other is a note for whoever picks the
  // work up next. Finding out afterwards is finding out too late.
  const user = userEvent.setup();
  stub({ steerable: true });
  render(<Review tick={0} />);
  await openReview(user);
  expect(await screen.findByText(/still working/)).toBeDefined();
});

it("says so when the agent has finished and the comments will become a slice", async () => {
  const user = userEvent.setup();
  stub({ steerable: false });
  render(<Review tick={0} />);
  await openReview(user);
  expect(await screen.findByText(/puts your comments on the plan/)).toBeDefined();
});

it("reports what submitting actually did", async () => {
  const user = userEvent.setup();
  stub();
  render(<Review tick={0} />);
  await openReview(user);

  await user.click(await screen.findByText("Request changes"));
  expect(await screen.findByText(/Sent 2 comment\(s\) to the agent/)).toBeDefined();
});

it("says whether the orchestrator heard it too, not just the author", async () => {
  // The author fixes the code; the orchestrator decides what the work is. A correction that
  // only reaches one leaves the plan still saying the old thing.
  const user = userEvent.setup();
  stub();
  render(<Review tick={0} />);
  await openReview(user);

  await user.click(await screen.findByText("Request changes"));
  expect(await screen.findByText(/The orchestrator has it too/)).toBeDefined();
});

it("shows existing comments against the line they belong to, and resolves them", async () => {
  const user = userEvent.setup();
  const calls = stub({
    comments: [
      {
        id: 9,
        review_id: 3,
        parent_id: null,
        file_path: "src/lib.rs",
        side: "new",
        line_start: 11,
        line_end: 11,
        author: "human",
        body: "rename x",
        status: "open",
        created_at: "",
      },
    ],
  });
  render(<Review tick={0} />);
  await openReview(user);

  expect(await screen.findByText("rename x")).toBeDefined();
  await user.click(screen.getByText("Resolve"));
  await waitFor(() =>
    expect(calls.some((call) => call.url === "/comments/9/resolve")).toBe(true),
  );
});

it("does not render a binary file as text", async () => {
  // git decides what is binary; showing its bytes is how a review becomes a screenful of
  // noise.
  const user = userEvent.setup();
  stub({
    files: [{ ...FILE, path: "logo.png", binary: true, hunks: [] }],
  });
  render(<Review tick={0} />);
  await openReview(user);
  expect(await screen.findByText(/Binary file/)).toBeDefined();
});

it("says plainly when there is nothing to review", async () => {
  stub({}, []);
  render(<Review tick={0} />);
  expect(await screen.findByText("Nothing to review.")).toBeDefined();
});

it("starts a minified file's diff collapsed, says why, and shows it on request", async () => {
  // A committed bundle is one line tens of thousands of characters long. Rendered, it
  // buried every other file of the PR under a wall of wrapped code.
  const bundle = `var e=${"x".repeat(20000)};`;
  const minified = {
    path: "ui/dist/app.js",
    old_path: null,
    status: "modified",
    binary: false,
    additions: 1,
    deletions: 1,
    hunks: [
      {
        header: "@@ -1,1 +1,1 @@",
        old_start: 1,
        new_start: 1,
        lines: [
          { kind: "removed", old: 1, new: null, text: "var e=1;" },
          { kind: "added", old: null, new: 1, text: bundle },
        ],
      },
    ],
  };
  stub({ files: [FILE, minified] });
  const user = userEvent.setup();
  render(<Review tick={0} />);
  await openReview(user);

  // The file anybody would read is shown as ever.
  expect(await screen.findByText("fn two(x: i32) {}")).toBeDefined();
  expect(screen.getByText("ui/dist/app.js")).toBeDefined();
  expect(screen.getByText(/minified or generated/)).toBeDefined();
  expect(screen.queryByText(bundle)).toBeNull();

  await user.click(screen.getByRole("button", { name: "Show diff" }));
  expect(screen.getByText(bundle)).toBeDefined();
});
