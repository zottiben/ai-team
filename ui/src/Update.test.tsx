import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { UpdateBanner } from "./Update";

afterEach(() => {
  vi.unstubAllGlobals();
});

function stub(available: Record<string, unknown>, applyFails?: string) {
  const calls: { url: string; method: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      calls.push({ url, method: init?.method ?? "GET" });
      if (init?.method === "POST") {
        if (applyFails !== undefined) {
          return Promise.resolve({
            ok: false,
            status: 400,
            json: async () => ({ error: applyFails }),
          });
        }
        return Promise.resolve({
          ok: true,
          json: async () => ({ version: "0.2.0", restart_required: true }),
        });
      }
      return Promise.resolve({
        ok: true,
        json: async () => ({
          current: "0.1.0",
          latest: "0.2.0",
          method: "release",
          can_update: true,
          blocked: null,
          ...available,
        }),
      });
    }),
  );
  return calls;
}

it("offers an update when there is one", async () => {
  stub({});
  render(<UpdateBanner />);
  expect(await screen.findByText(/0\.2\.0 is available/)).toBeDefined();
});

it("says nothing at all when there is nothing to install", async () => {
  // A banner saying "up to date" is a banner nobody needs.
  stub({ latest: "0.1.0", can_update: false });
  const { container } = render(<UpdateBanner />);
  await waitFor(() => expect(container.querySelector(".update")).toBeNull());
});

it("does not offer an update it cannot perform", async () => {
  // Worse than saying nothing: it invites a click that ends in an error.
  stub({ can_update: false, method: "unknown", blocked: "not installed by its own installer" });
  const { container } = render(<UpdateBanner />);
  await waitFor(() => expect(container.querySelector(".update")).toBeNull());
});

it("asks for a restart, because a replaced binary is not a restarted process", async () => {
  // The one outcome worth preventing is a window that looks updated and is not.
  const user = userEvent.setup();
  stub({});
  render(<UpdateBanner />);

  await user.click(await screen.findByText("Update"));
  expect(await screen.findByText(/Restart ai-team/)).toBeDefined();
  expect(screen.getByText(/Updated to 0\.2\.0/)).toBeDefined();
});

it("reports a failed update and lets it be tried again", async () => {
  const user = userEvent.setup();
  stub({}, "could not replace /usr/local/bin/ait");
  render(<UpdateBanner />);

  await user.click(await screen.findByText("Update"));
  expect(await screen.findByText(/could not replace/)).toBeDefined();
  // Back to offering, not stuck on "Updating…".
  expect(screen.getByText("Update").closest("button")?.disabled).toBe(false);
});

it("a check it cannot make is silence, not an error", async () => {
  // ai-team works perfectly well offline, and a red message about GitHub says otherwise.
  vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("network down")));
  const { container } = render(<UpdateBanner />);
  await waitFor(() => expect(container.querySelector(".update")).toBeNull());
  expect(container.querySelector(".error")).toBeNull();
});
