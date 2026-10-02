import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { ToolboxManagement } from "./ToolboxManagement";
import { OperationPreview } from "./ToolboxOperation";
import type { Operation, Registration } from "./toolbox-operations-api";
import { useState } from "react";

const registration: Registration = { project: 1, name: "Repository", revision: 1, status: "active", next_status: "active", restore_status: null, roots: [{ path: "/isolated/repo", exists: true }], reason: null };
const initial: Operation = { id: 9, authority: { kind: "user", home: "/isolated/home", pi_agent: "/isolated/pi", charter_targets: ["/explicit/rules.md"] }, changes: [{ root: "/explicit", effects: [{ path: "rules.md", summary: "append charter", before: { kind: "missing" }, after: { kind: "file", mode: 420, contents: [110, 101, 119] } }] }], registrations: [], scan_roots: null, warnings: [], state: "preview", outcome: null };
const item = { key: "pre-pr", contents: "source", description: null };
afterEach(() => vi.unstubAllGlobals());
function stub(failApply = false) {
  let current = initial;
  const calls: { url: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
    const url = String(input), body = init?.body ? JSON.parse(String(init.body)) : null;
    calls.push({ url, body });
    let response: unknown;
    if (url.endsWith("/catalogue")) response = { revision: "pinned", skills: [item], hooks: [], mcp: [], rules: [], templates: [], helpers: [], charter: item, notice: "bundled" };
    else if (url.endsWith("/toolbox/user")) response = initial.authority;
    else if (url.endsWith("/toolbox/registry")) response = { roots: [], projects: [registration] };
    else if (url.endsWith("/discover")) response = { roots: ["/isolated"], repositories: [{ path: "/isolated/new", project: null }], warnings: [] };
    else if (url.endsWith("/preview")) { current = url.includes("registry") ? { ...initial, authority: { kind: "registry" }, changes: [], registrations: [{ ...registration, next_status: "archived" }] } : initial; response = current; }
    else if (url.endsWith("/apply")) {
      if (failApply) return { ok: false, status: 409, text: async () => "Another operation is applying" };
      current = { ...current, state: "applied", outcome: { applied: ["exact path"], problem: null, uncertain: false } }; response = current;
    } else if (/\/operations\/(user|registry)\/9$/.test(url)) response = current;
    else if (/\/operations\/(user|registry)$/.test(url)) response = [];
    else throw new Error(`Unexpected request ${url}`);
    return { ok: true, json: async () => response };
  }));
  return calls;
}
it("requires separate user approval, names custom authority, and invalidates changed choices", async () => {
  const user = userEvent.setup(), calls = stub();
  render(<ToolboxManagement mode="user" onChanged={() => {}} onRegistered={() => {}} />);
  await user.click(await screen.findByLabelText("pre-pr"));
  await user.click(screen.getByText("Preview user setup"));
  const apply = await screen.findByText("Apply scoped approval");
  expect((apply as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/Explicit charter destinations.*explicit\/rules.md/)).toBeDefined();
  await user.click(screen.getByLabelText(/I approve this exact USER scope/));
  await user.click(screen.getByLabelText("Append base charter to global rules"));
  expect(screen.queryByText("Apply scoped approval")).toBeNull();
  await user.type(screen.getByLabelText("Custom charter file (optional absolute path)"), "/explicit/rules.md");
  await user.click(screen.getByText("Preview user setup"));
  await user.click(await screen.findByLabelText(/I approve this exact USER scope/));
  await user.click(screen.getByText("Apply scoped approval"));
  await screen.findByRole("status");
  expect(calls.filter((c) => c.url.endsWith("/apply"))).toEqual([{ url: "/api/toolbox/operations/user/9/apply", body: {} }]);
  expect(calls.every((c) => !c.url.includes("/projects/"))).toBe(true);
  expect(calls.filter((c) => c.url.endsWith("/preview")).at(-1)?.body).toEqual({ harnesses: ["pi"], skills: ["pre-pr"], no_symlink: false, charter: true, charter_path: "/explicit/rules.md" });
});
it("discovers read-only, then separately previews metadata-only forgetting", async () => {
  const user = userEvent.setup(), calls = stub(), changed = vi.fn();
  render(<ToolboxManagement mode="registry" onChanged={changed} onRegistered={() => {}} />);
  await user.type(screen.getByLabelText("One absolute directory per line"), "/isolated");
  await user.click(screen.getByText("Scan selected roots"));
  await screen.findByText("Add discovered repository");
  expect(calls.filter((c) => c.body !== null)).toEqual([{ url: "/api/toolbox/discover", body: { roots: ["/isolated"] } }]);
  await user.click(screen.getByText("Preview forgetting Repository"));
  await screen.findByText(/Repository: active → archived/);
  await user.click(screen.getByLabelText(/I approve this exact REGISTRY scope/));
  await user.click(screen.getByText("Apply scoped approval"));
  await waitFor(() => expect(changed).toHaveBeenCalledOnce());
  expect(calls.some((c) => c.url === "/api/toolbox/registry/preview" && JSON.stringify(c.body) === '{"operation":"forget","projects":[1]}')).toBe(true);
});
function Receipt() {
  const [preview, update] = useState(initial), [busy, setBusy] = useState(false);
  return <OperationPreview preview={preview} onUpdate={update} busy={busy} setBusy={setBusy} />;
}
it("re-reads a refused claim rather than inventing an interrupted receipt or retrying", async () => {
  const user = userEvent.setup(), calls = stub(true); render(<Receipt />);
  await user.click(screen.getByLabelText(/I approve this exact USER scope/));
  await user.click(screen.getByText("Apply scoped approval"));
  await screen.findByRole("alert");
  expect(screen.queryByText(/Applying or interrupted/)).toBeNull();
  expect((screen.getByText("Apply scoped approval") as HTMLButtonElement).disabled).toBe(true);
  expect(calls.filter((c) => c.url.endsWith("/apply"))).toHaveLength(1);
  expect(calls.some((c) => c.url === "/api/toolbox/operations/user/9")).toBe(true);
});
