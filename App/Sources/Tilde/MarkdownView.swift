import AppKit
import TildeFFI
import WebKit

/// Markdown drawn by Tilde, so it looks the same on every Mac and scrolls with Tilde's
/// keys. Both ways start from the core's parse. `native` turns its blocks into attributed
/// text: instant (10–30 ms), it's what the pane shows. `web` puts its HTML in a web view
/// styled from the theme: tables, images, HTML and maths as on GitHub, for PREVIEW
/// (`[preview] markdown`).
@MainActor
enum MarkdownRender {
    // MARK: Web

    /// The page around the core's HTML. No scripts run and nothing is fetched (the core
    /// embeds local images and drops remote ones; the policy below enforces it).
    static func page(body: String, appearance: NSAppearance) -> String {
        let theme = Theme.shared
        let c = theme.colors
        var css = ""
        appearance.performAsCurrentDrawingAppearance {
            let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            css = """
            :root { color-scheme: \(dark ? "dark" : "light"); }
            html { background: \(hex(c.background)); }
            body { margin: 0; padding: 18px 28px 48px; color: \(hex(c.text));
              font: \(theme.list.pointSize)px/1.6 -apple-system, system-ui, sans-serif;
              -webkit-font-smoothing: antialiased; overflow-wrap: break-word; }
            h1, h2, h3, h4, h5, h6 { line-height: 1.25; margin: 1.4em 0 0.6em; }
            body > :first-child { margin-top: 0; }
            h1 { font-size: 1.8em; } h2 { font-size: 1.4em; } h3 { font-size: 1.15em; }
            h1, h2 { padding-bottom: 0.3em; border-bottom: 1px solid \(hex(c.border)); }
            p, ul, ol, pre, table, blockquote { margin: 0 0 1em; }
            a { color: \(hex(c.match)); text-decoration: none; }
            a:hover { text-decoration: underline; }
            code, pre { font-family: '\(theme.list.familyName ?? "Menlo")', ui-monospace, monospace; font-size: 0.88em; }
            :not(pre) > code { background: \(hex(c.bar)); padding: 0.15em 0.4em; border-radius: 5px; }
            pre { background: \(hex(c.bar)); padding: 12px 16px; border-radius: 8px; overflow-x: auto; line-height: 1.45; }
            blockquote { padding: 0 1em; color: \(hex(c.textDim)); border-left: 3px solid \(hex(c.border)); }
            table { border-collapse: collapse; display: block; overflow-x: auto; }
            th, td { border: 1px solid \(hex(c.border)); padding: 6px 13px; }
            th { background: \(hex(c.bar)); font-weight: 600; }
            img { max-width: 100%; }
            hr { border: 0; border-top: 1px solid \(hex(c.border)); margin: 1.6em 0; }
            ul, ol { padding-left: 1.7em; } li + li { margin-top: 0.25em; }
            li:has(> input[type=checkbox]) { list-style: none; }
            li > input[type=checkbox] { margin: 0 0.45em 0 -1.4em; }
            .img-alt { display: inline-block; font-size: 0.8em; line-height: 1.5; color: \(hex(c.textDim));
              border: 1px solid \(hex(c.border)); border-radius: 4px; padding: 0 0.45em; margin: 0 0.15em; }
            ::selection { background: \(hex(c.selection)); }
            math { font-family: 'STIX Two Math', math; font-size: 1.12em; }
            math[display=block] { display: block; margin: 0.9em 0; overflow-x: auto; overflow-y: hidden; }
            """
        }
        return """
        <!doctype html><html><head><meta charset="utf-8">
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'">
        <style>\(css)</style></head><body>\(body)</body></html>
        """
    }

    private static func hex(_ color: NSColor) -> String {
        guard let c = color.usingColorSpace(.sRGB) else { return "inherit" }
        let v = [c.redComponent, c.greenComponent, c.blueComponent].map { Int(($0 * 255).rounded()) }
        return String(format: "rgba(%d,%d,%d,%.3f)", v[0], v[1], v[2], c.alphaComponent)
    }

    static func makeWebView(links: WebLinks) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        config.websiteDataStore = .nonPersistent()
        let web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = links
        // No white flash before the page's own background draws.
        web.setValue(false, forKey: "drawsBackground")
        return web
    }

    /// Scrolls the page. Scripts from the page are off, but the app's own still run.
    static func scroll(_ web: WKWebView, _ how: PreviewPane.Scroll) {
        let js = switch how {
        case .down: "window.scrollBy({top: 56, behavior: 'smooth'})"
        case .up: "window.scrollBy({top: -56, behavior: 'smooth'})"
        case .pageDown: "window.scrollBy({top: window.innerHeight - 56, behavior: 'smooth'})"
        case .pageUp: "window.scrollBy({top: 56 - window.innerHeight, behavior: 'smooth'})"
        case .top: "window.scrollTo({top: 0, behavior: 'smooth'})"
        case .bottom: "window.scrollTo({top: document.body.scrollHeight, behavior: 'smooth'})"
        }
        web.evaluateJavaScript(js, completionHandler: nil)
    }

    // MARK: Native

    /// The blocks as text for an NSTextView that wraps to `width`, with body text at `size`
    /// (headings scale from it).
    static func attributed(_ blocks: [MarkdownBlock], width: CGFloat, size: CGFloat = Theme.shared.list.pointSize) -> NSAttributedString {
        let theme = Theme.shared
        let c = theme.colors
        let body = NSFont.systemFont(ofSize: size)
        let mono = NSFont(descriptor: theme.list.fontDescriptor, size: size * 0.9) ?? theme.list
        let out = NSMutableAttributedString()
        let indentStep: CGFloat = 22

        func font(_ run: MarkdownRun, base: NSFont) -> NSFont {
            var f = run.code ? mono : base
            if run.bold { f = NSFontManager.shared.convert(f, toHaveTrait: .boldFontMask) }
            if run.italic { f = NSFontManager.shared.convert(f, toHaveTrait: .italicFontMask) }
            return f
        }

        for (i, block) in blocks.enumerated() {
            let style = NSMutableParagraphStyle()
            style.lineHeightMultiple = 1.18
            style.paragraphSpacing = size * 0.75
            let quoteIndent = CGFloat(block.quote) * 18
            style.firstLineHeadIndent = quoteIndent
            style.headIndent = quoteIndent
            var base = body
            var color = block.quote > 0 ? c.textDim : c.text
            var prefix = ""
            var codeBlock: NSTextBlock?

            switch block.kind {
            case .heading:
                let scale: [CGFloat] = [1.8, 1.4, 1.17, 1.05, 1, 0.95]
                base = .systemFont(ofSize: size * scale[min(Int(block.level), 6) - 1], weight: .bold)
                style.paragraphSpacingBefore = i == 0 ? 0 : size * 0.9
                style.paragraphSpacing = size * 0.5
            case .listItem:
                let indent = quoteIndent + CGFloat(max(Int(block.level) - 1, 0)) * indentStep
                style.firstLineHeadIndent = indent
                style.headIndent = indent + indentStep
                style.tabStops = [NSTextTab(textAlignment: .left, location: indent + indentStep)]
                style.paragraphSpacing = size * 0.3
                prefix = block.marker + "\t"
            case .paragraph where block.level > 0:
                // A paragraph inside a list item lines up with the item's text.
                let indent = quoteIndent + CGFloat(block.level) * indentStep
                style.firstLineHeadIndent = indent
                style.headIndent = indent
                style.paragraphSpacing = size * 0.3
            case .code, .table:
                // One box for all its lines (each line is a paragraph, so the spacing
                // between blocks is the box's margin, not paragraph spacing).
                let box = NSTextBlock()
                box.backgroundColor = c.bar
                box.setWidth(12, type: .absoluteValueType, for: .padding)
                box.setWidth(size * 0.75, type: .absoluteValueType, for: .margin, edge: .maxY)
                box.setValue(100, type: .percentageValueType, for: .width)
                codeBlock = box
                style.lineHeightMultiple = 1.1
                style.paragraphSpacing = 0
            case .rule:
                let line = NSTextBlock()
                line.setWidth(1, type: .absoluteValueType, for: .border, edge: .maxY)
                line.setBorderColor(c.border, for: .maxY)
                line.setValue(100, type: .percentageValueType, for: .width)
                codeBlock = line
                style.paragraphSpacingBefore = size * 0.4
            default:
                break
            }
            if codeBlock == nil, block.quote > 0 {
                // A bar down the left of quotes.
                let bar = NSTextBlock()
                bar.setWidth(3, type: .absoluteValueType, for: .border, edge: .minX)
                bar.setBorderColor(c.border, for: .minX)
                bar.setWidth(12, type: .absoluteValueType, for: .padding, edge: .minX)
                bar.setWidth(CGFloat(block.quote - 1) * 18, type: .absoluteValueType, for: .margin, edge: .minX)
                bar.setValue(100, type: .percentageValueType, for: .width)
                style.firstLineHeadIndent -= quoteIndent
                style.headIndent -= quoteIndent
                codeBlock = bar
            }
            if let codeBlock { style.textBlocks = [codeBlock] }

            let start = out.length
            if !prefix.isEmpty {
                out.append(NSAttributedString(string: prefix, attributes: [.font: base, .foregroundColor: c.textDim]))
            }
            switch block.kind {
            case .image:
                if let path = block.image, let image = NSImage(contentsOfFile: path), image.size.width > 0 {
                    let maxWidth = min(max(width - 60, 80), block.imageWidth.map { CGFloat($0) } ?? .greatestFiniteMagnitude)
                    let scale = min(1, maxWidth / image.size.width)
                    let attachment = NSTextAttachment()
                    attachment.image = image
                    attachment.bounds = CGRect(x: 0, y: 0, width: image.size.width * scale, height: image.size.height * scale)
                    out.append(NSAttributedString(attachment: attachment))
                } else {
                    let alt = block.runs.map(\.text).joined()
                    out.append(NSAttributedString(
                        string: "▨ " + (alt.isEmpty ? "image" : alt),
                        attributes: [.font: body, .foregroundColor: c.textDim]))
                }
            case .rule:
                out.append(NSAttributedString(string: "\u{200B}", attributes: [.font: NSFont.systemFont(ofSize: 2)]))
            default:
                if block.kind == .code || block.kind == .table { base = mono; color = c.text }
                for run in block.runs {
                    var attrs: [NSAttributedString.Key: Any] = [
                        .font: font(run, base: base),
                        .foregroundColor: run.color.map { NSColor(rgba: $0) } ?? color,
                    ]
                    if run.code, block.kind != .code, block.kind != .table { attrs[.backgroundColor] = c.bar }
                    if run.strike { attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
                    if let link = run.link, let url = URL(string: link) {
                        attrs[.link] = url
                        attrs[.foregroundColor] = c.match
                    }
                    out.append(NSAttributedString(string: run.text, attributes: attrs))
                }
            }
            if i < blocks.count - 1 { out.append(NSAttributedString(string: "\n")) }
            out.addAttribute(.paragraphStyle, value: style, range: NSRange(location: start, length: out.length - start))
        }
        return out
    }
}

/// Links in the web view open in the browser (or the app for their kind), never in the pane.
final class WebLinks: NSObject, WKNavigationDelegate {
    var onLoad: (() -> Void)?

    func webView(
        _ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
        decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void
    ) {
        if action.navigationType == .linkActivated, let url = action.request.url {
            if url.scheme != nil, url.scheme != "about" { NSWorkspace.shared.open(url) }
            return decisionHandler(.cancel)
        }
        decisionHandler(action.request.url?.scheme == "about" ? .allow : .cancel)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        onLoad?()
    }
}
