import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { Toolbox } from "./Toolbox";
import type { SetupPreview, ToolboxScan, ToolboxCatalogue } from "./toolbox-api";

const scan: ToolboxScan = {
  root: "/isolated/project", catalogue_revision: "90659e82", problem: null, worktrees: [], worktree_problem: null,
  survey: { state: "unconfigured", harnesses: [], items: [], findings: [], inventory: { agents_md: false, claude_md: false, pi: { adapter_servers: [] } }, recommendation: { detected: ["react"], hooks: ["format-on-edit"], skills: ["pre-pr"], mcp: ["context7"], rules: [], notes: ["No automatic software installation."] } },
};
const item = (key: string, contents = `Reference ${key}`) => ({ key, contents, description: null });
const catalogue: ToolboxCatalogue = { revision: "90659e82", hooks: [item("format-on-edit"), item("guard-irreversible")], mcp: [item("context7"), item("pixellab")], skills: [item("pre-pr"), item("cli/gh")], rules: [item("unity", "Keep engine files safe")], templates: [item("templates/RULES.template.md")], helpers: [item("with-dotenv.sh")], charter: item("base-charter"), notice: "Bundled licences" };
const preview: SetupPreview = { id: 42, project_id: 7, root: scan.root, catalogue_revision: "90659e82", state: "preview", warnings: ["Review exact bytes"], outcome: null, effects: [{ path: "AGENTS.md", summary: "scaffold", before: { kind: "missing" }, after: { kind: "file", contents: [110, 101, 119], mode: 420 } }] };
afterEach(() => vi.unstubAllGlobals());
function stub(stale = false) {
  const calls: { url: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, body: init?.body ? JSON.parse(String(init.body)) : null });
    return { ok: true, json: async () => url.endsWith("/apply") ? { ...preview, state: stale ? "refused" : "applied", outcome: { applied: stale ? [] : ["AGENTS.md"], problem: stale ? "toolbox preview is stale" : null, uncertain: false } } : url.endsWith("/preview") ? preview : url.endsWith("/history") ? [] : url.endsWith("/catalogue") ? catalogue : [scan] };
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
it("offers non-recommended catalogue items, copy-mode and environment setup", async () => {
  const user = userEvent.setup(); const calls = stub();
  render(<Toolbox project={7} initial={scan} />);
  await user.click(await screen.findByLabelText("cli/gh"));
  await user.click(screen.getByLabelText("pixellab"));
  await user.click(screen.getByLabelText(/independent Claude skill copies/));
  await user.click(screen.getByLabelText(/Install the .env launcher/));
  await user.click(screen.getByText("Preview selected setup"));
  await screen.findByText("Approve and apply");
  expect(calls.find((c) => c.url.endsWith("/preview"))?.body).toEqual({ root: scan.root, selection: {
    operation: "install", harnesses: ["pi"], hooks: ["format-on-edit"], mcp: ["context7", "pixellab"], skills: ["pre-pr", "cli/gh"], scaffold: true, no_symlink: true, with_dotenv: true,
  } });
  await user.click(screen.getByText("Select recommended items"));
  expect(screen.queryByText("Approve and apply")).toBeNull();
  expect((screen.getByLabelText("cli/gh") as HTMLInputElement).checked).toBe(false);
});
it("browses and copies rules/templates without file mutation, even when scan failed", async () => {
  const user = userEvent.setup(); const calls = stub();
  render(<Toolbox project={7} initial={{ ...scan, survey: null, problem: "Inspect this checkout" }} />);
  await user.click(await screen.findByText("Browse bundled catalogue, rules and templates"));
  await user.type(screen.getByLabelText("Search catalogue"), "unity");
  await user.click(screen.getByText("unity"));
  expect(screen.getByText("Keep engine files safe")).toBeDefined();
  await user.click(screen.getByText("Copy unity"));
  expect(await screen.findByText("Copied unity. No files changed.")).toBeDefined();
  expect(await navigator.clipboard.readText()).toBe("Keep engine files safe");
  expect(calls.every((c) => c.body === null)).toBe(true);
  expect(screen.queryByText("Approve and apply")).toBeNull();
  await user.clear(screen.getByLabelText("Search catalogue"));
  expect(screen.getByText("templates/RULES.template.md")).toBeDefined();
});
it("previews layout migration separately from the selected install choices", async () => {
  const user = userEvent.setup(); const calls = stub();
  render(<Toolbox project={7} initial={scan} />);
  await user.click(screen.getByText("Preview layout migration"));
  await screen.findByText("Approve and apply");
  expect(calls.find((c) => c.url.endsWith("/preview"))?.body).toEqual({ root: scan.root, selection: { operation: "migrate" } });
  expect(calls.some((c) => c.url.endsWith("/apply"))).toBe(false);
  await user.click(screen.getByLabelText(/independent Claude skill copies/));
  expect(screen.queryByText("Approve and apply")).toBeNull();
});
it("exposes a scan error rather than offering blind repair", () => {
  stub(); render(<Toolbox project={7} initial={{ ...scan, survey: null, problem: "External skills symlink needs manual inspection" }} />);
  expect(screen.getByRole("alert").textContent).toContain("manual inspection");
  expect(screen.queryByText("Preview selected setup")).toBeNull();
});
