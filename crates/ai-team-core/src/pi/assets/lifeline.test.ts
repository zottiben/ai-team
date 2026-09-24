// The lifeline, tested with real processes on both CI legs (D12).
//
// What it exists for is a supervisor that dies without unwinding, so that is what the
// first test does: a stand-in for ai-team starts a turn the way ai-team does - leading a
// process group of its own, read through a pipe - and is then killed outright.
//
//   node --test crates/ai-team-core/src/pi/assets/lifeline.test.ts

import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Readable } from "node:stream";
import { test } from "node:test";
import { pathToFileURL } from "node:url";

import { holdOn } from "./lifeline.ts";

/** Whether a process is still there. A zombie is not: it is gone, only not yet reaped. */
function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
  } catch {
    return false;
  }
  try {
    return !execFileSync("ps", ["-o", "stat=", "-p", String(pid)], { encoding: "utf8" })
      .trim()
      .startsWith("Z");
  } catch {
    return false;
  }
}

async function until(done: () => boolean, within: number): Promise<boolean> {
  const deadline = Date.now() + within;
  while (Date.now() < deadline) {
    if (done()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return done();
}

function firstLine(stream: Readable): Promise<string> {
  return new Promise((resolve, reject) => {
    let text = "";
    stream.on("data", (chunk: Buffer) => {
      text += chunk.toString();
      const end = text.indexOf("\n");
      if (end >= 0) resolve(text.slice(0, end));
    });
    stream.on("end", () => reject(new Error(`no line before the stream ended: ${text}`)));
  });
}

test("a turn whose supervisor dies stops, and takes what it started with it", async () => {
  const dir = mkdtempSync(join(tmpdir(), "ait-lifeline-"));
  const lifeline = pathToFileURL(join(import.meta.dirname, "lifeline.ts")).href;
  // The turn: holds on, starts a tool in its own group, and would otherwise run forever.
  writeFileSync(
    join(dir, "turn.ts"),
    `import { spawn } from "node:child_process";
import { holdOn } from ${JSON.stringify(lifeline)};
holdOn({ every: 50 });
const tool = spawn("sleep", ["60"], { stdio: "ignore" });
process.stdout.write(process.pid + " " + tool.pid + "\\n");
setInterval(() => {}, 1000);
`,
  );
  // The supervisor: starts the turn leading a group of its own and reads it, as ai-team does.
  writeFileSync(
    join(dir, "supervisor.ts"),
    `import { spawn } from "node:child_process";
const turn = spawn(process.execPath, [${JSON.stringify(join(dir, "turn.ts"))}], {
  detached: true,
  stdio: ["ignore", "pipe", "inherit"],
});
turn.stdout.pipe(process.stdout);
setInterval(() => {}, 1000);
`,
  );
  const supervisor = spawn(process.execPath, [join(dir, "supervisor.ts")], {
    stdio: ["ignore", "pipe", "inherit"],
  });
  let turn = 0;
  try {
    const [started, tool] = (await firstLine(supervisor.stdout)).split(" ").map(Number);
    turn = started;
    assert.ok(alive(turn) && alive(tool), "the turn and its tool are running");

    // A crash: nothing in the supervisor gets to stop what it started.
    supervisor.kill("SIGKILL");

    assert.ok(await until(() => !alive(turn), 5000), "the turn is still running");
    assert.ok(await until(() => !alive(tool), 5000), "the turn's tool is still running");
  } finally {
    supervisor.kill("SIGKILL");
    if (turn > 0) {
      try {
        process.kill(-turn, "SIGKILL");
      } catch {
        // Already gone, which is the point.
      }
    }
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a turn whose supervisor is still there carries on", async () => {
  let stopped = 0;
  const letGo = holdOn({ stop: () => stopped++, parent: () => 4242, every: 10 });
  await new Promise((resolve) => setTimeout(resolve, 100));
  letGo();
  assert.equal(stopped, 0);
});

test("a turn stops once, however long it goes on noticing", async () => {
  let asked = 0;
  let stopped = 0;
  // The supervisor for the first look, and a new parent from then on.
  const parent = () => (asked++ === 0 ? 4242 : 1);
  const letGo = holdOn({ stop: () => stopped++, parent, every: 10 });
  await new Promise((resolve) => setTimeout(resolve, 100));
  letGo();
  assert.equal(stopped, 1);
});
