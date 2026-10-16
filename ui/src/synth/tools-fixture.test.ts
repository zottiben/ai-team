import { webcrypto } from "node:crypto";
import { afterEach, expect, it, vi } from "vitest";
import { toolsFixture } from "./tools-fixture";
afterEach(() => vi.unstubAllGlobals());
const url = (path: string) => new URL(`http://fixture/api${path}?project=demo&workspace=%2Ffixture%2Fdemo&path=calc.ts`);
it("limits synthetic saves to an enabled exact in-memory preview", async () => {
  vi.stubGlobal("crypto", webcrypto);
  for (const enabled of [false, true]) {
    const tools = toolsFixture(enabled);
    const opened = await tools.request(url("/editor/open"), "GET", null);
    const data = opened?.data as { revision: unknown };
    const body = { project: "demo", workspace: "/fixture/demo", path: "calc.ts", base: data.revision, text: "export const total = 2;\n" };
    const preview = await tools.request(url("/editor/preview"), "POST", body);
    const captured = preview?.data as { proposed: { hash: string } };
    expect((await tools.request(url("/editor/save"), "POST", { ...body, proposed_hash: "wrong" }))?.status).toBe(enabled ? 409 : 403);
    expect(tools.text("calc.ts")).toBe("export const total = 1;\n");
    expect((await tools.request(url("/editor/save"), "POST", { ...body, proposed_hash: captured.proposed.hash }))?.status).toBe(enabled ? 200 : 403);
    expect(tools.commands).toHaveLength(enabled ? 1 : 0);
    expect(await tools.request(new URL("http://fixture/api/editor/open?project=other"), "GET", null)).toBeNull();
  }
});
it("keeps terminal input literal and rejects implicit process creation", async () => {
  const tools = toolsFixture(true);
  expect(await tools.request(url("/terminals"), "POST", { project: "demo", workspace: "/fixture/demo" })).toBeNull();
  await tools.request(url("/terminals/7"), "POST", { text: "echo $(do-not-run)\r" });
  expect(tools.output()).toContain("echo $(do-not-run)\r\nfixture> ");
  expect((await toolsFixture(false).request(url("/terminals/7"), "POST", { text: "anything" }))?.status).toBe(403);
  expect((await tools.request(url("/terminals/7/resize"), "POST", { rows: 0, cols: 80 }))?.status).toBe(400);
});
