import { beforeEach, describe, expect, it, vi } from "vitest";

import { api, captureToken, token } from "./api";

function locationFor(href: string): Location {
  return { href } as Location;
}

function fakeHistory(): { history: History; replaced: string[] } {
  const replaced: string[] = [];
  const history = {
    replaceState: (_state: unknown, _title: string, url: string) => {
      replaced.push(url);
    },
  } as unknown as History;
  return { history, replaced };
}

describe("captureToken", () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it("takes the token out of the URL and scrubs the query", () => {
    const { history, replaced } = fakeHistory();

    const captured = captureToken(locationFor("http://127.0.0.1:7788/?token=abc123"), history);

    expect(captured).toBe("abc123");
    expect(token()).toBe("abc123");
    // A credential left in the address bar gets bookmarked and pasted.
    expect(replaced).toEqual(["/"]);
  });

  it("keeps the rest of the URL intact", () => {
    const { history, replaced } = fakeHistory();

    captureToken(locationFor("http://127.0.0.1:7788/console?view=graph&token=abc#step-2"), history);

    expect(replaced).toEqual(["/console?view=graph#step-2"]);
  });

  it("falls back to what the tab already knows on reload", () => {
    const { history, replaced } = fakeHistory();
    sessionStorage.setItem("ai-team.token", "stored");

    expect(captureToken(locationFor("http://127.0.0.1:7788/"), history)).toBe("stored");
    expect(replaced).toEqual([]);
  });

  it("is null when there is no token anywhere", () => {
    const { history } = fakeHistory();
    expect(captureToken(locationFor("http://127.0.0.1:7788/"), history)).toBeNull();
  });
});

describe("api", () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it("sends the token as a header", async () => {
    sessionStorage.setItem("ai-team.token", "abc123");
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ version: "0.1.0" }),
    });
    vi.stubGlobal("fetch", fetchMock);

    await expect(api("/health")).resolves.toEqual({ version: "0.1.0" });
    expect(fetchMock).toHaveBeenCalledWith("/api/health", {
      headers: { "x-ai-team-token": "abc123" },
    });
  });

  it("surfaces the server's own message rather than a bare status", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 401,
        json: async () => ({ error: "unauthorized" }),
      }),
    );

    await expect(api("/health")).rejects.toThrow("unauthorized");
  });
});
