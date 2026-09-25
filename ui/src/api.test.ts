import { beforeEach, describe, expect, it, vi } from "vitest";

import { api, captureToken, doctor, post, token } from "./api";

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

describe("doctor", () => {
  it("asks once for callers that ask at the same time, and afresh after", async () => {
    // A report is a few seconds of subprocesses, and the window asks for one twice as it
    // opens - to decide on Setup, and for the health banner - so the two ran side by side
    // and the second waited on the first.
    let answer: (value: unknown) => void = () => {};
    const fetchMock = vi.fn(
      () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const first = doctor();
    const second = doctor();
    answer({ ok: true, json: async () => ({ severity: "ok" }) });
    await expect(Promise.all([first, second])).resolves.toEqual([
      { severity: "ok" },
      { severity: "ok" },
    ]);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    // Not a cache: the next question is asked again, because the machine may have changed.
    const later = doctor();
    answer({ ok: true, json: async () => ({ severity: "blocking" }) });
    await expect(later).resolves.toEqual({ severity: "blocking" });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    vi.unstubAllGlobals();
  });
});

describe("doctor after a write", () => {
  it("does not hand a report read before a change to somebody asking after it", async () => {
    // A fix or a provider change is followed by a fresh read; sharing one begun before the
    // write would say the thing just fixed is still broken.
    const answers: Array<(value: unknown) => void> = [];
    const fetchMock = vi.fn(
      (_path: string, init?: RequestInit) =>
        init?.method === "POST"
          ? Promise.resolve({ ok: true, json: async () => ({ done: "fixed" }) })
          : new Promise((resolve) => answers.push(resolve)),
    );
    vi.stubGlobal("fetch", fetchMock);

    const before = doctor();
    await post("/doctor/fix", { action: "reseat_stranded" });
    const after = doctor();
    expect(answers).toHaveLength(2);

    answers[0]!({ ok: true, json: async () => ({ severity: "blocking" }) });
    answers[1]!({ ok: true, json: async () => ({ severity: "ok" }) });
    await expect(before).resolves.toEqual({ severity: "blocking" });
    await expect(after).resolves.toEqual({ severity: "ok" });
    vi.unstubAllGlobals();
  });
});
