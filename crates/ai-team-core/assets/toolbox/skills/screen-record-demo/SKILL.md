---
name: screen-record-demo
description: Record a short demo (20-60s) of a running web UI, driving the page yourself with a synthetic cursor so the interaction reads. Covers picking a capture route that actually works, getting to mp4 with no ffmpeg, the choreography rules that stop a take being wasted, and how to prove the file captured. Use when asked for a demo video, screen recording, or walkthrough of an implementation.
---

# screen-record-demo - record a demo of a running UI

Driving the page yourself gives a repeatable, precisely paced demo. It also has
sharp edges that waste whole takes. Pick the capture route before promising
anything.

## Pick the capture route first

**Check whether the browser has a real window before assuming you can screen-record
it.** Agent-driven browsers usually do not.

```bash
osascript -e 'tell application "System Events" to tell (first process whose unix id is <PID>) to return (count of windows)'
screencapture -x /tmp/f.png   # then read it - wallpaper means no window is visible
```

| | Route A - `recordVideo` | Route B - `screencapture -v` |
| --- | --- | --- |
| Needs a visible window | no | **yes** |
| Captures | the page viewport | the whole screen |
| Output | webm, needs converting | mp4 direct |
| Leaks desktop / menu bar | never | always |

**Route A is the default.** Both MCP browsers (`chrome-devtools`, `playwright`) run
headless - `count of windows` is 0 - so `screencapture` records only the wallpaper.
Launching a headed Chrome yourself does not reliably help either: Playwright passes
`--no-startup-window` (removable via `ignoreDefaultArgs`), but on some machines the
window still never reaches the desktop session. Verify, do not assume.

Route A's framing is better anyway - cropped to the app, no menu bar, no other
windows. Its only cost is the conversion below.

Route B is worth it only when the demo must show browser chrome or another app.
It records the **whole screen** (`-R` does not combine with `-v`) and needs Screen
Recording permission for the terminal; without it, it exits silently and writes no
file. Test permission before any setup:
`screencapture -v -V 2 /tmp/t.mp4 >/dev/null 2>&1; sleep 5; ls -lh /tmp/t.mp4`

Either way, **CDP input does not move the real macOS pointer**, so a recording of
CDP-driven clicks shows the UI changing with nothing visibly causing it. Inject
`demo-cursor.js` from beside this file.

## Output: mp4, and where it goes

Deliver **`.mp4`**. QuickTime, Finder, Spotlight and GitHub PR comments all reject
webm. Ask the user where recordings belong and honour it (Ben's is `~/Screenshots/`);
name the file after the ticket and what it shows. Delete the intermediate webm.

There is usually **no system ffmpeg**. If Playwright is installed there is a
stripped build at `~/Library/Caches/ms-playwright/ffmpeg-*/ffmpeg-mac` which
**decodes anything but encodes VP8 only** - no libx264, no videotoolbox - and has
~24 filters with **no `fps`** (seek per frame with `-ss` instead; `scale` works).
macOS `avconvert` cannot read webm at all.

Route to h264 with nothing installed:

```bash
FF=~/Library/Caches/ms-playwright/ffmpeg-1011/ffmpeg-mac
"$FF" -loglevel error -i in.webm -t <secs> -c copy trimmed.webm   # trim the dead tail
"$FF" -loglevel error -i trimmed.webm /tmp/frames/%05d.png        # ~4s for 1400 frames
swiftc -O -o /tmp/pngs2mp4 <skill-dir>/pngs2mp4.swift             # /usr/bin/swiftc, no Xcode
/tmp/pngs2mp4 /tmp/frames ~/Screenshots/<name>.mp4 25             # AVFoundation h264
```

Offer `brew install ffmpeg` as a one-command alternative, but do not install it
without asking.

## Procedure

1. **Get the app running and reachable.** Project-specific - use the project's own
   bring-up or e2e skill.
2. **Log in once and save `storageState`** to a file, then reuse it for every take.
   Re-shoots become cheap, and you touch any credential only once.
3. **Launch and record** (Route A), sized to a clean 16:10-ish viewport:
   ```js
   const context = await browser.newContext({
     storageState: STATE,
     ignoreHTTPSErrors: true,          // self-signed local certs
     viewport: { width: 1560, height: 940 },
     deviceScaleFactor: 2,
     recordVideo: { dir: OUT, size: { width: 1560, height: 940 } },
   });
   ```
   Recording starts at context creation and the file is only written on
   `context.close()`.
4. **Position the page.** Find the real scroll container first - pages often scroll
   inside a div, so `window.scrollTo` silently does nothing. Walk up from the target
   for `overflowY: auto|scroll` with `scrollHeight > clientHeight`, then set
   `scrollTop`. Leave room below anything that opens downward, or it gets clamped
   over its own trigger.
5. **Inject the cursor** from `demo-cursor.js` (`page.evaluate(script)`, or paste the
   body inline into `evaluate_script` for the MCP - its `filePath` argument saves
   output and cannot load a script).
6. **Write the choreography** as one async `run()` on `window.__demo`, per the rules
   below. Call it **without awaiting**, then `page.waitForTimeout(<budget + 8s>)`.
7. **Verify** - see below. **Restore** anything you changed: dev servers, symlinks,
   credentials, window state.

## Choreography rules

- **Wrap the body in try/catch** that records to a marker. An uncaught throw ends
  the run silently and you get a half-length video with no error anywhere.
- **Do not mark progress via `document.title`** - the app's router overwrites it.
  Use a plain global (`window.__demoMarks`) and read it before closing the context.
- **Resolve elements at click time**, never from a snapshot taken earlier. The DOM
  changes under you as panels open.
- **Read the real markup before writing selectors.** Guessing costs a take: MUI
  `ListItemButton` is a `div[role="button"]`, `MenuItem` is `li[role="menuitem"]`.
  Open the panel once and dump `innerText` of the candidates.
- **Click, do not hover, to change state.** Hovering a category or tab renders
  nothing new, so the option you want next will not exist.
- **Typed input must be committed.** Type character by character (~90ms) with the
  native setter plus an `input` event, then fire **`Enter`** - a synthetic `blur`
  does nothing because React listens to `focusout`:
  ```js
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
  setter.call(el, el.value + ch);
  el.dispatchEvent(new Event('input', { bubbles: true }));
  // ...then
  el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  ```
- **Never hardcode dates, months or row positions.** Read what the app is showing
  (a heading, an `aria-label`) and derive from it. A date that does not exist in the
  rendered view kills the run.
- **Move, pulse, then click.** ~700-900ms to travel, 1200-1800ms to dwell after a
  state change, so a viewer can read what happened.
- **Show it working on real data.** A demo that filters an empty table proves
  nothing. Find seeded data first and choose ranges that return rows.
- **Caption each beat** (`say`/`hush` in `demo-cursor.js`). Captions carry the
  narrative and let you keep the video short.
- **Budget the time.** Sum your waits, aim for 20-60s, and trim the tail afterwards
  rather than cutting the ending short.
- **End on a clean, complete state.** A half-finished selection makes the final
  frame look broken.

## Verify - never assume it captured

- Your completion marker is present, not a `FAILED:` entry.
- The final DOM state, and any network calls you asserted on, match what the demo
  should have produced.
- **Read frames out of the finished mp4 and look at them.** File size proves
  nothing; a black recording is still megabytes.
  ```bash
  qlmanage -t -s 1500 -o /tmp/thumb "<file>.mp4"    # QuickLook = macOS can decode it
  mdls -name kMDItemDurationSeconds -name kMDItemCodecs "<file>.mp4"
  ```
  `mdls` returning `(null)` means macOS cannot read the file - you have not
  delivered a usable video.

Report honestly that the cursor and captions are synthetic, and that Route A is a
**viewport capture, not a screen capture**, so nobody is surprised when they share it.

## Pitfalls

- Repointing a shared dev-server symlink leaves PHP's realpath cache serving the old
  target for ~2 minutes, so the page silently shows **another checkout's code**.
  Poll a known-different URL until it flips before you record.
- Browsers cache the served bundle hard. A stale document can pin the page to
  another worktree's dev server with no error - clear it via CDP
  (`Network.setCacheDisabled`, `Network.clearBrowserCache`) and navigate with a
  `?cb=<timestamp>`.
- `pkill -f "<generic pattern>"` kills other checkouts' dev servers too. Match on
  the absolute path, or resolve each PID's cwd with `lsof -p <pid> -a -d cwd -Fn`.
- `playwright_browser_run_code_unsafe` takes `async (page) => {...}`; a bare
  statement body is a syntax error. Batch several steps per call - far faster than
  snapshot-click-snapshot.
- The `chrome-devtools` MCP can wedge on "browser is already running" even with zero
  Chrome processes and the profile `Singleton*` locks deleted. That is its own state
  and needs the MCP server restarted; switch to Playwright rather than fighting it.
- Expect to redo a take. Verify after each one rather than stacking changes.
- `tell application "Finder" to get bounds of window of desktop` can hang for
  minutes. Read `screen.width`/`screen.height` from the page instead.
