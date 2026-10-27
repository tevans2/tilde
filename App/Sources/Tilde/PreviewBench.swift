#if DEBUG
import AppKit
import Quartz
@preconcurrency import QuickLookThumbnailing
import TildeFFI
import WebKit

/// Development aid: `TILDE_PREVIEW_BENCH=/a.md,/b.md` times each way of previewing a file,
/// from "selected" to "drawn in the window", and prints a table:
///
/// - bat: bat's first `max_lines`, as the pane shows code and text
/// - native: Tilde's Markdown as attributed text
/// - web (new view) / web (reused): Tilde's Markdown in a web view, until loaded and painted
/// - QL thumbnail: Quick Look's thumbnail, as the pane's instant stage
/// - QL full: Quick Look's preview view, until its content is attached
///
/// Each runs 6 times: the first is cold, the median of the rest is warm.
@MainActor
enum PreviewBench {
    private static let runs = 6
    private static let size = NSSize(width: 700, height: 750)

    static func runIfRequested() -> Bool {
        guard let list = ProcessInfo.processInfo.environment["TILDE_PREVIEW_BENCH"] else { return false }
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "Tilde preview bench"
        window.makeKeyAndOrderFront(nil)
        // The browser window stays (closing it ends its services), just out of the way.
        for other in NSApp.windows where other !== window { other.orderOut(nil) }
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            await run(list.split(separator: ",").map(String.init), in: window)
            NSApp.terminate(nil)
        }
        return true
    }

    private static func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

    private static func run(_ paths: [String], in window: NSWindow) async {
        let host = window.contentView!
        // The view holds its navigation delegate weakly: keep one alive for the reused view.
        let warmLinks = WebLinks()
        let warmWeb = MarkdownRender.makeWebView(links: warmLinks)
        let quickLook = QLPreviewView(frame: host.bounds, style: .normal)!
        let methods: [(String, (String) async -> Void)] = [
            ("bat", { await bat($0, host) }),
            ("native", { await native($0, host) }),
            ("web (new view)", { await web($0, host, reuse: nil) }),
            ("web (reused)", { _ = warmLinks; await web($0, host, reuse: warmWeb) }),
            ("QL thumbnail", { await thumbnail($0, host) }),
            ("QL full", { await full($0, host, quickLook) }),
        ]
        var out = "method               first ms   warm median ms\n"
        for path in paths {
            let size = (try? FileManager.default.attributesOfItem(atPath: path)[.size] as? Int) ?? 0
            out += "\n\((path as NSString).lastPathComponent)  (\(size / 1024) KB)\n"
            for (name, method) in methods {
                var times: [Double] = []
                for _ in 0..<runs {
                    host.subviews.forEach { $0.removeFromSuperview() }
                    let start = now()
                    await method(path)
                    times.append(Double(now() - start) / 1e6)
                    try? await Task.sleep(for: .milliseconds(50))
                }
                let warm = times.dropFirst().sorted()
                out += "  " + name.padding(toLength: 18, withPad: " ", startingAt: 0)
                    + String(format: " %8.1f   %8.1f\n", times[0], warm[warm.count / 2])
            }
        }
        FileHandle.standardError.write(out.data(using: .utf8)!)
    }

    /// Lays out and draws the window now, as the next screen refresh would.
    private static func draw(_ view: NSView) {
        view.window?.contentView?.layoutSubtreeIfNeeded()
        view.window?.displayIfNeeded()
        CATransaction.flush()
    }

    private static func textView(_ host: NSView, wrap: Bool) -> NSTextView {
        let scroll = NSScrollView(frame: host.bounds)
        let text = NSTextView(frame: host.bounds)
        text.isEditable = false
        text.drawsBackground = false
        if !wrap {
            text.isHorizontallyResizable = true
            text.textContainer?.widthTracksTextView = false
            text.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        }
        scroll.documentView = text
        host.addSubview(scroll)
        return text
    }

    private static func bat(_ path: String, _ host: NSView) async {
        let theme = Theme.shared.batThemeDark
        let lines = UInt32(Theme.shared.batMaxLines)
        guard let preview = await Task.detached(operation: { batPreview(path: path, theme: theme, maxLines: lines) }).value
        else { return }
        let text = textView(host, wrap: false)
        text.textStorage?.setAttributedString(PreviewPane.attributed(preview))
        draw(text)
    }

    private static func native(_ path: String, _ host: NSView) async {
        guard let blocks = await Task.detached(operation: { markdownBlocks(path: path, dark: true) }).value else { return }
        let text = textView(host, wrap: true)
        text.textStorage?.setAttributedString(MarkdownRender.attributed(blocks, width: host.bounds.width))
        draw(text)
    }

    private static func web(_ path: String, _ host: NSView, reuse: WKWebView?) async {
        guard let html = await Task.detached(operation: { markdownHtml(path: path, dark: true) }).value else { return }
        let links = WebLinks()
        let web = reuse ?? MarkdownRender.makeWebView(links: links)
        web.frame = host.bounds
        host.addSubview(web)
        await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            var resumed = false
            links.onLoad = {
                guard !resumed else { return }
                resumed = true
                done.resume()
            }
            web.navigationDelegate = links
            web.loadHTMLString(MarkdownRender.page(body: html, appearance: host.effectiveAppearance), baseURL: nil)
        }
        withExtendedLifetime(links) {}
        // Loaded isn't painted: wait for two animation frames.
        _ = try? await web.callAsyncJavaScript(
            "await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))",
            contentWorld: .defaultClient)
    }

    private static func thumbnail(_ path: String, _ host: NSView) async {
        let request = QLThumbnailGenerator.Request(
            fileAt: URL(fileURLWithPath: path), size: host.bounds.size, scale: 2,
            representationTypes: [.lowQualityThumbnail, .thumbnail])
        guard let image = try? await QLThumbnailGenerator.shared.generateBestRepresentation(for: request).nsImage
        else { return }
        let view = NSImageView(frame: host.bounds)
        view.image = image
        host.addSubview(view)
        draw(view)
    }

    private static func full(_ path: String, _ host: NSView, _ quickLook: QLPreviewView) async {
        quickLook.previewItem = nil
        quickLook.frame = host.bounds
        host.addSubview(quickLook)
        quickLook.previewItem = URL(fileURLWithPath: path) as NSURL
        let start = now()
        while !quickLook.subviews.contains(where: { !$0.subviews.isEmpty }), now() - start < 3_000_000_000 {
            try? await Task.sleep(for: .milliseconds(2))
        }
    }
}
#endif
