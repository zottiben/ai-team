import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Projects } from "./Projects";

afterEach(() => {
  vi.unstubAllGlobals();
});

function stub(options: {
  projects?: unknown[];
  checks?: unknown[];
  registerFails?: string;
} = {}) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      const method = init?.method ?? "GET";
      calls.push({
        url,
        method,
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });

      if (url === "/projects" && method === "POST") {
        if (options.registerFails !== undefined) {
          return Promise.resolve({
            ok: false,
            status: 400,
            json: async () => ({ error: options.registerFails }),
          });
        }
        return Promise.resolve({
          ok: true,
          json: async () => ({
            project: { id: 1, slug: "widget", name: "Widget", kind: "repo" },
            repo_path: "/Users/me/Developer/widget",
            created: true,
            seeded_team: true,
            roster: [],
          }),
        });
      }
      if (url === "/projects") {
        return Promise.resolve({ ok: true, json: async () => options.projects ?? [] });
      }
      if (url === "/doctor") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            version: "0.1.0",
            checks: options.checks ?? [],
            severity: "fine",
            can_run: true,
            needs_setup: false,
          }),
        });
      }
      return Promise.resolve({ ok: true, json: async () => ({ attached: "/x" }) });
    }),
  );
  return calls;
}

const WIDGET = { id: 1, slug: "widget", name: "Widget", kind: "repo", status: "active", open_runs: 0 };

it("adding a project sends the path and reports what it did", async () => {
  const user = userEvent.setup();
  const calls = stub();
  render(<Projects onChanged={() => {}} />);

  await user.type(await screen.findByLabelText("path"), "/Users/me/Developer/widget");
  await user.click(screen.getByText("Add"));

  await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
  expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({
    path: "/Users/me/Developer/widget",
  });
  // Said, rather than left to be inferred from a list refreshing.
  expect(await screen.findByText(/Added Widget with a team of six/)).toBeDefined();
});

it("a name is optional and not sent when blank", async () => {
  // The directory is what somebody standing in it would call the project.
  const user = userEvent.setup();
  const calls = stub();
  render(<Projects onChanged={() => {}} />);

  await user.type(await screen.findByLabelText("path"), "/Users/me/Developer/widget");
  await user.click(screen.getByText("Add"));

  await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
  expect((calls.find((c) => c.method === "POST")?.body as { name?: string }).name).toBeUndefined();
});

it("shows what a project is missing, from the readiness report", async () => {
  // Rather than working it out again here, and rather than discovering it when a run fails
  // to lease a checkout that has moved.
  stub({
    projects: [WIDGET],
    checks: [
      {
        id: "project.widget",
        label: "Project: Widget",
        severity: "blocking",
        detail: "/old/path no longer exists",
        fix: { by: "human", what: "Point Widget at where the checkout is now" },
      },
    ],
  });
  render(<Projects onChanged={() => {}} />);

  expect(await screen.findByText("/old/path no longer exists")).toBeDefined();
  expect(screen.getByText(/Point Widget at where the checkout is now/)).toBeDefined();
  expect(screen.getByText("blocking")).toBeDefined();
});

it("a healthy project shows no warning", async () => {
  stub({
    projects: [WIDGET],
    checks: [
      {
        id: "project.widget",
        label: "Project: Widget",
        severity: "fine",
        detail: "/Users/me/Developer/widget",
        fix: { by: "none" },
      },
    ],
  });
  render(<Projects onChanged={() => {}} />);
  await screen.findByText("Widget");
  expect(screen.queryByText("blocking")).toBeNull();
});

it("attaching a second repo is offered on every project", async () => {
  // A project is a container: a change spanning a service and its client is one piece of
  // work.
  const user = userEvent.setup();
  const calls = stub({ projects: [WIDGET] });
  render(<Projects onChanged={() => {}} />);

  await user.click(await screen.findByText("Attach another repo"));
  await user.type(screen.getByLabelText("repo path for 1"), "/Users/me/Developer/client");
  await user.click(screen.getByText("Attach"));

  await waitFor(() => expect(calls.some((c) => c.url === "/projects/1/repos")).toBe(true));
  expect(calls.find((c) => c.url === "/projects/1/repos")?.body).toEqual({
    path: "/Users/me/Developer/client",
  });
});

it("a path that will not register says why and keeps what was typed", async () => {
  const user = userEvent.setup();
  stub({ registerFails: "/nope is not a directory" });
  render(<Projects onChanged={() => {}} />);

  const input = await screen.findByLabelText("path");
  await user.type(input, "/nope");
  await user.click(screen.getByText("Add"));

  expect(await screen.findByText(/is not a directory/)).toBeDefined();
  expect((input as HTMLInputElement).value).toBe("/nope");
});

it("works on a machine with no database yet", async () => {
  // This page is where the first project comes from, so a 503 from /projects is the
  // expected state rather than an error worth showing.
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const url = String(input).replace(/^\/api/, "");
      if (url === "/doctor") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            version: "0.1.0",
            checks: [],
            severity: "blocking",
            can_run: false,
            needs_setup: true,
          }),
        });
      }
      return Promise.resolve({
        ok: false,
        status: 503,
        json: async () => ({ error: "no database" }),
      });
    }),
  );
  render(<Projects onChanged={() => {}} />);

  expect(await screen.findByText(/No projects yet/)).toBeDefined();
  expect(screen.queryByText(/no database/)).toBeNull();
});
