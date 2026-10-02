import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { Toolbox } from "./Toolbox";
import type { SetupPreview, ToolboxScan } from "./toolbox-api";

const scan: ToolboxScan = {
  root: "/isolated/project", catalogue_revision: "90659e82", problem: null, worktrees: [], worktree_problem: null,
  survey: { state: "unconfigured", harnesses: [], items: [], findings: [], inventory: { agents_md: false, claude_md: false, pi: { adapter_servers: [] } }, recommendation: { detected: ["react"], hooks: ["format-on-edit"], skills: ["pre-pr"], mcp: ["context7"], rules: [], notes: ["No automatic software installation."] } },
};
const preview: SetupPreview = { id: 42, project_id: 7, root: scan.root, catalogue_revision: "90659e82", state: "preview", warnings: ["Review exact bytes"], outcome: null, effects: [{ path: "AGENTS.md", summary: "scaffold", before: { kind: "missing" }, after: { kind: "file", contents: [110, 101, 119], mode: 420 } }] };
afterEach(() => vi.unstubAllGlobals());
function stub(stale = false) {
  const calls: { url: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, body: init?.body ? JSON.parse(String(init.body)) : null });
    return { ok: true, json: async () => url.endsWith("/apply") ? { ...preview, state: stale ? "refused" : "applied", outcome: { applied: stale ? [] : ["AGENTS.md"], problem: stale ? "toolbox preview is stale" : null, uncertain: false } } : url.endsWith("/preview") ? preview : url.endsWith("/history") ? [] : [scan] };
  }));
  return calls;
}
it("keeps scanning read-only and requires exact preview plus explicit approval", async () => {
  const user = userEvent.setup();
  const calls = stub();
  render(<Toolbox project={7} initial={scan} />);
  expect(calls.every((call) => call.body === null)).toBe(true);
  expect(screen.queryByText("Approve and apply")).toBeNull();
  await user.click(screen.getByText("Preview selected setup"));
  await screen.findByText(/1 exact file/);
  const approve = screen.getByText("Approve and apply");
  expect((approve as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByText(/AGENTS.md — scaffold/));
  expect(screen.getByText("new")).toBeDefined();
  expect(screen.getByText("Mode 644 · 3 bytes")).toBeDefined();
  await user.click(screen.getByLabelText(/I reviewed these exact/));
  await user.click(approve);
  expect(await screen.findByText(/Setup applied/)).toBeDefined();
  expect(calls.find((c) => c.url.endsWith("/apply"))).toEqual({ url: "/api/projects/7/toolbox/previews/42/apply", body: {} });
  expect(screen.queryByText("Approve and apply")).toBeNull();
});
it("invalidates the displayed approval when choices change", async () => {
  const user = userEvent.setup(); stub();
  render(<Toolbox project={7} initial={scan} />);
  await user.click(screen.getByText("Preview selected setup"));
  await screen.findByText("Approve and apply");
  await user.click(screen.getByLabelText("context7"));
  expect(screen.queryByText("Approve and apply")).toBeNull();
});
it("reports stale refusal without retrying or reconstructing the approved setup", async () => {
  const user = userEvent.setup(); const calls = stub(true);
  render(<Toolbox project={7} initial={scan} />);
  await user.click(screen.getByText("Preview selected setup"));
  await screen.findByText("Approve and apply");
  await user.click(screen.getByLabelText(/I reviewed these exact/));
  await user.click(screen.getByText("Approve and apply"));
  expect(await screen.findByText("toolbox preview is stale")).toBeDefined();
  expect(screen.getByText("0 changes applied.")).toBeDefined();
  expect(screen.queryByText("Approve and apply")).toBeNull();
  await waitFor(() => expect(calls.filter((c) => c.url.endsWith("/preview"))).toHaveLength(1));
});
it("exposes a scan error rather than offering blind repair", () => {
  stub(); render(<Toolbox project={7} initial={{ ...scan, survey: null, problem: "External skills symlink needs manual inspection" }} />);
  expect(screen.getByRole("alert").textContent).toContain("manual inspection");
  expect(screen.queryByText("Preview selected setup")).toBeNull();
});
