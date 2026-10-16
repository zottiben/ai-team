import AppKit
import WebKit
@MainActor final class Probe: NSObject, NSApplicationDelegate, WKNavigationDelegate {
    var window: NSWindow!; var web: WKWebView!; var phase = Int(ProcessInfo.processInfo.environment["SYNTH_PROBE_START"] ?? "0") ?? 0; var completed = 0
    let cases = [
        (1280, "team-overview=true&theme=dark", "/synth-patch.checks.js", "exercisePatchBay"),
        (390, "tools=true&tool-input=true&theme=light", "/synth-tools.checks.js", "exerciseTools"),
        (1280, "theme=dark", "/synth-shell-flow.js", "exerciseWorkspace"),
        (390, "theme=light", "/synth-composer.checks.js", "exerciseComposer"),
        (1280, "activity=5000&theme=dark", "/synth-tracker.checks.js", "exerciseTracker"),
        (390, "activity=5000&activity-mode=reads&theme=light", "/synth-tracker.checks.js", "exerciseGroupedTracker"),
        (1280, "activity=5000&activity-mode=thinking&theme=dark", "/synth-tracker.checks.js", "exerciseGroupedTracker"),
        (390, "activity=5000&theme=light", "/synth-tracker.checks.js", "exerciseOutput"),
        (1280, "activity=5000&activity-motion=true&theme=dark", "/synth-arrival.checks.js", "exerciseReceiptMotion"),
        (390, "activity=5000&activity-motion=true&theme=light", "/synth-arrival.checks.js", "exerciseReceiptMotion"),
        (1280, "overview=true&theme=dark", "/synth-overview.checks.js", "exerciseOverview"),
        (390, "review=true&theme=light", "/synth-review.checks.js", "exerciseReview"),
        (1280, "review=true&theme=dark", "/synth-review.checks.js", "exerciseApproval"),
        (390, "review=true&theme=light", "/synth-review.checks.js", "exerciseSyntax"),
        (1280, "review=true&team-review=true&theme=dark", "/synth-review.checks.js", "exerciseTeamReview"),
        (390, "work=true&theme=light", "/synth-work.checks.js", "exerciseWork"),
        (1280, "board=true&theme=dark", "/synth-planning.checks.js", "exercisePlanningRetention"),
        (390, "board=true&theme=light", "/synth-planning.checks.js", "exercisePlanHeader"),
        (1280, "board=true&board-input=true&theme=dark", "/synth-board-input.checks.js", "exercisePlanStatus"),
        (390, "board=true&board-input=true&theme=light", "/synth-board-input.checks.js", "exerciseProductionBoard"),
        (1280, "board=true&board-input=true&theme=dark", "/synth-board-input.checks.js", "exerciseBlockerCommands"),
        (390, "board=true&board-input=true&theme=light", "/synth-board-input.checks.js", "exerciseLostBlockerResponse"),
        (1280, "board=true&board-input=true&board-empty-active=true&theme=dark", "/synth-board-input.checks.js", "exerciseBoardMotion"),
        (390, "board=true&board-input=true&theme=light", "/synth-board-input.checks.js", "exerciseMotionOwnership"),
        (1280, "board=true&board-input=true&theme=dark", "/synth-board-input.checks.js", "exerciseBoardDisposal"),
        (1280, "/synth-preview.html?theme=dark", "/synth-theme.checks.js", "exerciseThemes"),
        (1280, "/synth-preview.html?theme=dark", "/synth-preview.checks.js", "checkPreview"),
        (390, "/synth-preview.html?theme=light", "/synth-preview.checks.js", "checkPreview")
    ]
    func fail(_ text: String) -> Never { FileHandle.standardError.write(Data(("WK FAIL: " + text + "\n").utf8)); exit(1) }
    func applicationDidFinishLaunching(_ notification: Notification) {
        let config = WKWebViewConfiguration(); config.websiteDataStore = .nonPersistent(); config.preferences.javaScriptCanOpenWindowsAutomatically = false
        web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1280, height: 900), configuration: config)
        web.navigationDelegate = self; web.autoresizingMask = [.width, .height]
        window = NSWindow(contentRect: web.frame, styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Isolated Synth probe · synthetic inputs only"; window.contentView = web; window.makeKeyAndOrderFront(nil); NSApp.activate(); load()
        DispatchQueue.main.asyncAfter(deadline: .now() + 240) { self.fail("timeout") }
    }
    func load() {
        guard let base = ProcessInfo.processInfo.environment["SYNTH_FIXTURE_URL"], base.hasPrefix("http://127.0.0.1:") else { fail("Use npm run test:native to start an isolated fixture server") }
        let c = cases[phase]; window.setContentSize(NSSize(width: c.0, height: 900))
        web.load(URLRequest(url: URL(string: base + (c.1.hasPrefix("/") ? c.1 : "/synth-shell-preview.html?" + c.1))!))
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { fail(String(describing: error)) }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Task { @MainActor in
            do {
                let status = try await web.evaluateJavaScript("JSON.stringify({visibility:document.visibilityState,url:location.href})")
                FileHandle.standardError.write(Data("WK status: \(status ?? "none")\n".utf8))
                guard let state = status as? String, state.contains("\"visibility\":\"visible\"") else { fail("Native viewport is not visible; visible verification blocked") }
                let c = cases[phase]
                let value = try await web.callAsyncJavaScript("return await (await import('\(c.2)')).\(c.3)();", arguments: [:], in: nil, contentWorld: .page)
                let data = try JSONSerialization.data(withJSONObject: value ?? [:], options: [.sortedKeys])
                FileHandle.standardOutput.write(Data(("WK synthetic \(c.3): " + String(decoding: data, as: UTF8.self) + "\n").utf8))
                phase += 1; completed += 1
                if phase == cases.count { FileHandle.standardOutput.write(Data("WK complete: \(completed) visible synthetic checks\n".utf8)); window.close(); exit(0) }; load()
            } catch {
                let debug = try? await web.evaluateJavaScript("JSON.stringify({top:document.querySelector('.chat-transcript')?.scrollTop,height:document.querySelector('.chat-transcript')?.scrollHeight,viewport:document.querySelector('.chat-transcript')?.clientHeight,follow:document.querySelector('.chat-follow')?.outerHTML,active:document.activeElement?.outerHTML.slice(0,300)})")
                FileHandle.standardError.write(Data("WK diagnostic: \(debug ?? "none")\n".utf8)); fail(String(describing: error))
            }
        }
    }
}
@main struct Main { @MainActor static func main() { let app = NSApplication.shared, probe = Probe(); app.delegate = probe; app.setActivationPolicy(.accessory); withExtendedLifetime(probe) { app.run() } } }
