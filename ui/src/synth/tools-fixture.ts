// Vite-only, in-memory files and a synthetic terminal buffer. Never opens a PTY or disk.
import type { FileRevision } from "../editor-api";
import type { TerminalCommand, TerminalPad } from "../api";
const initial = { "calc.ts": "export const total = 1;\n", "notes.md": "Synthetic editor and terminal fixture.\n" };
const bytes = (text: string) => new TextEncoder().encode(text);
async function revision(text: string): Promise<FileRevision> {
  const digest = await crypto.subtle.digest("SHA-256", bytes(text));
  return { hash: [...new Uint8Array(digest)].map(value => value.toString(16).padStart(2, "0")).join(""), bytes: bytes(text).length, device: 1, inode: 7, modified_ms: 1 };
}
export function toolsFixture(input: boolean, rich = false) {
  const pads: TerminalPad[] = rich ? ["Status", "Diff", "Tests", "Build", "Log"].map((label, position) => ({ id: position + 1, position, revision: 1, label, command: `fixture ${label.toLowerCase()} # synthetic, never executed` })) : [];
  const history: TerminalCommand[] = pads.slice(0, 3).map((pad, index) => ({ id: pad.id, request_id: `synthetic-${pad.id}`, token: `synthetic-${pad.id}`, pad_id: pad.id, revision: 1,
    label: pad.label, command: pad.command, worktree: "/fixture/demo", dispatch: "started", refusal: null, session_id: null,
    claimed_at: "2026-01-01T00:00:00Z", started_at: "2026-01-01T00:00:00Z", finished_at: "2026-01-01T00:00:01Z",
    exit_code: index === 2 ? 1 : 0, duration_ms: 1000, lines: 3, lines_exact: true, state: "finished" }));
  const baseline = new Map(Object.entries(initial)), disk = new Map(baseline), previews = new Map<string, string>();
  let output = "Synthetic terminal · no process is running.\r\nfixture> ";
  const commands: { path: string; body: Record<string, unknown> }[] = [];
  return { commands, text: (path: string) => disk.get(path), output: () => output,
    async request(url: URL, method: string, body: Record<string, unknown> | null) {
      const route = url.pathname.replace(/^\/api/, ""), path = url.searchParams.get("path") ?? String(body?.path ?? "");
      const ok = (data: unknown, status = 200) => ({ data, status });
      const scoped = (body?.project ?? url.searchParams.get("project")) === "demo" && (body?.workspace ?? url.searchParams.get("workspace")) === "/fixture/demo";
      if (method === "GET" && route === "/terminals/7") {
        const cursor = Number(url.searchParams.get("cursor") ?? 0);
        return ok({ text: output.slice(cursor), cursor: output.length, done: false, status: null, dropped: 0 });
      }
      if (method === "POST" && ["/terminals/7", "/terminals/7/resize"].includes(route)) {
        if (!input || !body) return ok({ error: "Synthetic terminal input requires tool-input=true." }, 403);
        if (route.endsWith("/resize")) {
          if (![body.rows, body.cols].every(value => typeof value === "number" && Number.isSafeInteger(value) && value > 0 && value <= 1000)) return ok({ error: "Invalid synthetic resize" }, 400);
        } else {
          if (typeof body.text !== "string" || body.text.length > 4096) return ok({ error: "Invalid synthetic input" }, 400);
          output += body.text.replaceAll("\r", "\r\nfixture> ");
        }
        commands.push({ path: route, body }); return ok({});
      }
      if (route === "/terminals/pads" && method === "GET" && url.searchParams.get("project") === "demo") return ok(pads);
      if (!scoped) return null;
      if (method === "GET") {
        if (route === "/tree") return ok(path ? [] : [...disk.keys()].map(path => ({ path, name: path, dir: false })));
        if (route === "/map") return ok({ root: "demo", files: 2, unowned: 2, truncated: false, zones: [], nodes: [...disk.keys()].map(path => ({ path, name: path, dir: false, depth: 1, weight: 1, owner: null })), edges: [] });
        if (route === "/scm") return ok({ branch: "fixture", branches: ["fixture"], unstaged: [], staged: [], untracked: [] });
        if (route === "/search") return ok([]);
        if (route === "/editor/kept") return ok([]);
        if (route === "/editor/baseline") return ok({ path, text: baseline.get(path) ?? null, tracked: disk.has(path) });
        if (route === "/editor/open" && disk.has(path)) { const text = disk.get(path)!; return ok({ path, text, editable: true, reason: null, revision: await revision(text), eol: "lf", line_separator: "\n", encoding: "utf-8", bytes: bytes(text).length }); }
        if (route === "/terminals") return ok([{ id: 7, worktree: "/fixture/demo", done: false, status: null, kind: "shell", label: "Synthetic session · no process", duration_ms: 0 }]);
        if (route === "/terminals/commands") return ok({ worktree: "/fixture/demo", running: [], history });
      }
      if (method === "POST") {
        if (route === "/lsp/diagnostics") return ok({ analysed: false, diagnostics: null });
        if (["/lsp/completion", "/lsp/definition"].includes(route)) return ok([]);
        if (route === "/lsp/hover") return ok(null);
        if (["/editor/preview", "/editor/save"].includes(route) && body && disk.has(path) && typeof body.text === "string" && body.text.length <= 65536) {
          const before = disk.get(path)!, current = await revision(before), proposed = await revision(body.text), stale = JSON.stringify(body.base) !== JSON.stringify(current);
          if (route === "/editor/preview") {
            const oldLines = before.split("\n"), newLines = body.text.split("\n");
            previews.set(proposed.hash, JSON.stringify({ path, base: body.base, text: body.text }));
            return ok({ checkout: "/fixture/demo", path, base: body.base, disk: current, stale, proposed: { hash: proposed.hash, bytes: proposed.bytes, eol: "lf" },
              change: { hunks: [{ before_start: 1, before_lines: oldLines.length, after_start: 1, after_lines: newLines.length, lines: [...oldLines.map((text, index) => ({ kind: "removed", before: index + 1, after: null, text })), ...newLines.map((text, index) => ({ kind: "added", before: null, after: index + 1, text }))] }], added: newLines.length, removed: oldLines.length, coarse: true, truncated: false }, disk_text: { text: before, truncated: false }, notices: ["In-memory synthetic preview; no filesystem writes."] });
          }
          if (!input) return ok({ error: "Synthetic saves require tool-input=true." }, 403);
          if (stale || body.proposed_hash !== proposed.hash || previews.get(proposed.hash) !== JSON.stringify({ path, base: body.base, text: body.text })) return ok({ reason: "not_the_preview", message: "Synthetic exact preview changed", disk: current, disk_text: { text: before, truncated: false }, preserved: [] }, 409);
          commands.push({ path: route, body }); disk.set(path, body.text);
          return ok({ path, revision: proposed, replaced: body.base, preserved: null, preserve_note: "This is in-memory fixture data, not a filesystem save.", notified_chats: 0 });
        }
      }
      return null;
    },
  };
}
