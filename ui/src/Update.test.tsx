import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { UpdateBanner, UpdateDetails, UPDATE_POLL_MS, useUpdates } from "./Update";
import type { Available } from "./api";

let status: Available;
let post = vi.fn<(init: RequestInit) => Promise<Response>>();
const fetcher = vi.fn();

function Fixture({ settings = false }: { settings?: boolean }) {
  const updates = useUpdates();
  return <><UpdateBanner updates={updates} />{settings && <UpdateDetails updates={updates} />}<textarea aria-label="Draft" defaultValue="Keep this unsent draft" /></>;
}

beforeEach(() => {
  status = {
    current: "0.7.5", latest: "0.7.6", method: "release", can_update: true, update_available: true, blocked: null,
    targets: [{ name: "CLI", path: "/fixture/bin/ait", version: "0.7.5", fingerprint: "old-cli" },
      { name: "Desktop app", path: "/fixture/ai-team.app", version: "0.7.5", fingerprint: "old-app" }],
    approval: "exact-reviewed-targets", checked_at: "now", state: "idle",
    release_url: "https://github.com/zottiben/ai-team/releases/tag/v0.7.6",
  };
  post = vi.fn(async () => new Response(JSON.stringify({ version: "0.7.6", restart_required: true, backup: "/fixture/backup" })));
  fetcher.mockReset().mockImplementation(async (_url: string, init?: RequestInit) => init?.method === "POST" ? post(init) : new Response(JSON.stringify(status)));
  vi.stubGlobal("fetch", fetcher);
});

afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("offers an update without installing it, changing the draft or navigating", async () => {
  render(<Fixture />);
  expect(await screen.findByText(/0.7.6 is available/)).toBeTruthy();
  expect(post).not.toHaveBeenCalled();
  expect((screen.getByRole("textbox", { name: "Draft" }) as HTMLTextAreaElement).value).toBe("Keep this unsent draft");
});

it("explains a lagging companion without claiming the running version is older", async () => {
  status = { ...status, latest: "0.7.5", targets: status.targets.map(target => target.name === "Desktop app" ? { ...target, version: "0.7.4" } : target) };
  render(<Fixture />);
  expect(await screen.findByText(/An installed program needs updating to 0.7.5/)).toBeTruthy();
  expect(screen.queryByText(/0.7.5 is available — you have 0.7.5/)).toBeNull();
  expect(post).not.toHaveBeenCalled();
});

it("shows no offer when all installed programs are up to date", async () => {
  status = { ...status, latest: "0.7.5", can_update: false, update_available: false, approval: null };
  render(<Fixture settings />);
  await screen.findByText(/Running 0.7.5/);
  expect(screen.queryByText(/is available/)).toBeNull();
  expect(post).not.toHaveBeenCalled();
});

it("reviews both concrete destinations and pins the explicit approval", async () => {
  render(<Fixture />);
  fireEvent.click(await screen.findByRole("button", { name: "Review update" }));
  expect(screen.getByText("/fixture/bin/ait")).toBeTruthy();
  expect(screen.getByText("/fixture/ai-team.app")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Update listed programs" }));
  await screen.findByText("Restart AI Team");
  expect(post).toHaveBeenCalledTimes(1);
  expect(post).toHaveBeenCalledWith(expect.objectContaining({ body: JSON.stringify({ version: "0.7.6", approval: "exact-reviewed-targets" }) }));
  expect((screen.getByRole("textbox", { name: "Draft" }) as HTMLTextAreaElement).value).toBe("Keep this unsent draft");
});

it("can replace a source build with an explicitly reviewed release", async () => {
  status.method = "source";
  render(<Fixture settings />);
  await screen.findByText(/replaces the local build with the reviewed published release/);
  expect(screen.getByRole("button", { name: "Update listed programs" })).not.toHaveProperty("disabled", true);
});

it("retains an installation error across the reconciliation read", async () => {
  post.mockResolvedValue(new Response(JSON.stringify({ error: "finish active work first" }), { status: 400 }));
  render(<Fixture settings />);
  await screen.findByText(/Running 0.7.5/);
  fireEvent.click(screen.getByRole("button", { name: "Update listed programs" }));
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "finish active work first");
  await waitFor(() => expect(fetcher).toHaveBeenCalledWith(expect.stringContaining("refresh=true"), expect.anything()));
  expect(screen.getByRole("alert").textContent).toBe("finish active work first");
  expect(screen.queryByText("Restart AI Team")).toBeNull();
});

it("keeps unsupported installation and network failures visible in Settings", async () => {
  status = { ...status, latest: null, can_update: false, approval: null, blocked: "could not check GitHub" };
  render(<Fixture settings />);
  await screen.findByText("could not check GitHub");
  expect(screen.getByRole("button", { name: "Update listed programs" })).toHaveProperty("disabled", true);
  expect(screen.queryByText(/up to date/)).toBeNull();
});

it("checks manually without ever using the install endpoint", async () => {
  render(<Fixture settings />);
  await screen.findByText(/Running 0.7.5/);
  fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(post).not.toHaveBeenCalled();
});

it("polls again and refreshes after returning, but does not install", async () => {
  vi.useFakeTimers();
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
  await act(async () => { render(<Fixture />); });
  expect(fetcher).toHaveBeenCalledTimes(1);
  status.latest = "0.7.7";
  await act(async () => { await vi.advanceTimersByTimeAsync(UPDATE_POLL_MS); });
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(screen.getByText(/0.7.7 is available/)).toBeTruthy();
  fireEvent(window, new Event("focus"));
  expect(fetcher).toHaveBeenCalledTimes(2);
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
  await act(async () => { await vi.advanceTimersByTimeAsync(UPDATE_POLL_MS); });
  expect(fetcher).toHaveBeenCalledTimes(2);
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
  await act(async () => { fireEvent(window, new Event("focus")); });
  expect(fetcher).toHaveBeenCalledTimes(3);
  expect(post).not.toHaveBeenCalled();
});

it("does not confuse the latest release with what another process installed", async () => {
  status = { ...status, latest: "0.7.9", state: "restart", can_update: false };
  render(<Fixture />);
  await screen.findByText(/The installed files changed/);
  expect(screen.queryByText(/Installed 0.7.9/)).toBeNull();
  expect(post).not.toHaveBeenCalled();
});

it("offers inspection instead of replaying an interrupted update", async () => {
  status = { ...status, state: "inspection", can_update: false, blocked: "mixed installation" };
  post.mockImplementation(async init => {
    expect(init.body).toBe("{}");
    status = { ...status, state: "idle", can_update: true, blocked: null };
    return new Response(JSON.stringify({ installed: false, version: "0.7.6", backup: "/fixture/backup", detail: "Original programs unchanged" }));
  });
  render(<Fixture settings />);
  fireEvent.click(await screen.findByRole("button", { name: "Inspect interrupted update" }));
  await waitFor(() => expect(screen.queryByText(/interrupted update needs inspection/)).toBeNull());
  expect(fetcher).toHaveBeenCalledWith(expect.stringContaining("/update/inspect"), expect.objectContaining({ method: "POST" }));
  expect(post).toHaveBeenCalledTimes(1);
  expect(screen.queryByText("Restart AI Team")).toBeNull();
});

it("survives StrictMode effect replay without losing the first check", async () => {
  render(<StrictMode><Fixture /></StrictMode>);
  await screen.findByText(/0.7.6 is available/);
  expect(post).not.toHaveBeenCalled();
});
