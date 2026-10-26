// Draws the Tilde app icon: a plain rounded square with one tilde in the middle, in a
// light and a dark variant (Catppuccin Latte and Mocha: base background, mauve tilde).
//
//   swift scripts/make-icon.swift App/Icon
//
// Writes Tilde-light.png and Tilde-dark.png (1024 px) into the folder given. Run
// scripts/install.sh afterwards to rebuild Tilde.icns.
import AppKit

struct Variant {
    let name: String
    let background: NSColor
    let tilde: NSColor
}

func hex(_ v: UInt32) -> NSColor {
    NSColor(srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255,
            blue: CGFloat(v & 0xff) / 255, alpha: 1)
}

let variants = [
    Variant(name: "light", background: hex(0xEFF1F5), tilde: hex(0x8839EF)),
    Variant(name: "dark", background: hex(0x1E1E2E), tilde: hex(0xCBA6F7)),
]

let size: CGFloat = 1024
/// The macOS icon grid: an 824 pt rounded square centred on the 1024 canvas.
let tile = NSRect(x: 100, y: 100, width: 824, height: 824)
let radius: CGFloat = 185

/// One period of a sine, trimmed at the ends so they curl like a typed tilde.
func tildePath() -> NSBezierPath {
    let path = NSBezierPath()
    let width: CGFloat = 430, amplitude: CGFloat = 92
    let from: CGFloat = -0.86, to: CGFloat = 0.86
    let steps = 240
    for i in 0...steps {
        let t = from + (to - from) * CGFloat(i) / CGFloat(steps)
        // Left hump up, right dip down (y grows upwards here).
        let point = NSPoint(x: size / 2 + t / to * width / 2, y: size / 2 - amplitude * sin(.pi * t))
        if i == 0 { path.move(to: point) } else { path.line(to: point) }
    }
    path.lineWidth = 104
    path.lineCapStyle = .round
    path.lineJoinStyle = .round
    return path
}

func render(_ v: Variant) -> Data {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0,
        bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    let shape = NSBezierPath(roundedRect: tile, xRadius: radius, yRadius: radius)
    // The soft drop shadow macOS icons carry.
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.28)
    shadow.shadowBlurRadius = 24
    shadow.shadowOffset = NSSize(width: 0, height: -10)
    shadow.set()
    v.background.setFill()
    shape.fill()
    NSGraphicsContext.restoreGraphicsState()

    // A hairline edge so the light tile doesn't melt into a light Dock.
    NSColor.black.withAlphaComponent(v.name == "light" ? 0.08 : 0.0).setStroke()
    shape.lineWidth = 2
    shape.stroke()

    v.tilde.setStroke()
    tildePath().stroke()

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let out = URL(fileURLWithPath: CommandLine.arguments.dropFirst().first ?? ".")
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
for v in variants {
    let file = out.appendingPathComponent("Tilde-\(v.name).png")
    try render(v).write(to: file)
    print("wrote \(file.path)")
}
