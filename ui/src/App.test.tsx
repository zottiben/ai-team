import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import App from "./App";

afterEach(() => {
  vi.unstubAllGlobals();
});

it("shows what the binary was built with", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ version: "0.1.0", bundle_embedded: true, bundle_files: 3 }),
    }),
  );

  render(<App />);

  expect(await screen.findByText("0.1.0")).toBeDefined();
  expect(await screen.findByText("3 files compiled in")).toBeDefined();
});

it("says so when the server cannot be reached", async () => {
  // The failure mode worth rendering: the page loaded from the bundle, so the binary is
  // fine, but the API refused it - which is almost always a stale token after a restart.
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status: 401,
      json: async () => ({ error: "unauthorized" }),
    }),
  );

  render(<App />);

  expect(await screen.findByText(/unauthorized/)).toBeDefined();
});
