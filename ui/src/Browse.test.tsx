import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Browse } from "./Browse";

afterEach(() => {
  vi.unstubAllGlobals();
});

type Listing = { path: string; parent: string | null; entries: unknown[] };

function stub(listings: Record<string, Listing>, failing: string[] = []) {
  const asked: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const path = new URL(String(input), "http://x").searchParams.get("path") ?? "";
      asked.push(path);
      if (failing.includes(path)) {
        return Promise.resolve({
          ok: false,
          status: 400,
          json: async () => ({ error: `${path} cannot be read` }),
        });
      }
      return Promise.resolve({ ok: true, json: async () => listings[path] });
    }),
  );
  return asked;
}

const HOME: Listing = {
  path: "/Users/me",
  parent: "/Users",
  entries: [
    { name: "widget", path: "/Users/me/widget", repo: true },
    { name: "src", path: "/Users/me/src", repo: false },
  ],
};

it("opens on home, because that is where a person's checkouts are", async () => {
  // Not `/`, which is a list of things none of them is.
  const asked = stub({ "": HOME });
  render(<Browse onPick={() => {}} />);

  expect(await screen.findByText("/Users/me")).toBeDefined();
  expect(asked).toEqual([""]);
});

it("says which directories are checkouts", async () => {
  stub({ "": HOME });
  render(<Browse onPick={() => {}} />);

  await screen.findByText("widget");
  // One badge, on the one that is a repository - not a decoration on every row.
  expect(screen.getAllByText("repo")).toHaveLength(1);
});

it("going into a directory and choosing one are different clicks", async () => {
  // A repository is usually both, so a single click would have to guess which was meant.
  const user = userEvent.setup();
  const picked: string[] = [];
  const asked = stub({
    "": HOME,
    "/Users/me/src": { path: "/Users/me/src", parent: "/Users/me", entries: [] },
  });
  render(<Browse onPick={(path) => picked.push(path)} />);

  await user.click(await screen.findByLabelText("choose widget"));
  expect(picked).toEqual(["/Users/me/widget"]);
  expect(asked, "choosing does not navigate").toEqual([""]);

  await user.click(screen.getByText("src"));
  await waitFor(() => expect(asked).toContain("/Users/me/src"));
  expect(picked, "entering a directory is not choosing it").toEqual(["/Users/me/widget"]);
});

it("the directory you are looking at can be chosen without entering another", async () => {
  const user = userEvent.setup();
  const picked: string[] = [];
  stub({ "": HOME });
  render(<Browse onPick={(path) => picked.push(path)} />);

  await user.click(await screen.findByText("Use this one"));
  expect(picked).toEqual(["/Users/me"]);
});

it("a directory that cannot be read leaves you where you were", async () => {
  // Rather than throwing you back to home with nothing to click, which is what resetting
  // the listing on failure would do.
  const user = userEvent.setup();
  stub({ "": HOME }, ["/Users/me/src"]);
  render(<Browse onPick={() => {}} />);

  await user.click(await screen.findByText("src"));

  expect(await screen.findByText(/cannot be read/)).toBeDefined();
  expect(screen.getByText("/Users/me")).toBeDefined();
  expect(screen.getByText("widget")).toBeDefined();
});

it("there is nowhere to go up to from the root", async () => {
  stub({ "": { path: "/", parent: null, entries: [] } });
  render(<Browse onPick={() => {}} />);

  const up = await screen.findByLabelText("up one directory");
  expect(up.hasAttribute("disabled")).toBe(true);
});
