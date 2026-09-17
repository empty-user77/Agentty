// Renders the DMG window background (1x and 2x) for Agentty's installer.
// Usage: swift scripts/make-dmg-background.swift <logo-mark.png> <out-dir>
// Layout (points, 660×420): app icon at (170, 220), Applications at (490, 220).

import AppKit

let args = CommandLine.arguments
guard args.count == 3, let mark = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("usage: make-dmg-background.swift <logo-mark.png> <out-dir>\n".data(using: .utf8)!)
    exit(1)
}
let out = URL(fileURLWithPath: args[2])
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

let width: CGFloat = 660, height: CGFloat = 420

func render(scale: CGFloat, name: String) {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(width * scale), pixelsHigh: Int(height * scale),
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = NSSize(width: width, height: height)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let bounds = NSRect(x: 0, y: 0, width: width, height: height)

    // Deep charcoal with a faint warm glow behind the drag path.
    NSGradient(colors: [NSColor(red: 0.09, green: 0.09, blue: 0.11, alpha: 1), NSColor(red: 0.13, green: 0.13, blue: 0.16, alpha: 1)])!
        .draw(in: bounds, angle: 90)
    let glow = NSGradient(colors: [NSColor(red: 0.99, green: 0.81, blue: 0.02, alpha: 0.10), NSColor(red: 0.99, green: 0.81, blue: 0.02, alpha: 0)])!
    glow.draw(fromCenter: NSPoint(x: width / 2, y: 200), radius: 0, toCenter: NSPoint(x: width / 2, y: 200), radius: 260, options: [])

    // Header: mark + name + tagline.
    mark.draw(in: NSRect(x: width / 2 - 88, y: height - 78, width: 34, height: 34))
    let title: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 24, weight: .semibold), .foregroundColor: NSColor.white]
    NSAttributedString(string: "Agentty", attributes: title).draw(at: NSPoint(x: width / 2 - 44, y: height - 75))
    let tagline: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12, weight: .regular), .foregroundColor: NSColor(white: 1, alpha: 0.55)]
    let tag = NSAttributedString(string: "The orchestration terminal for AI-native workflows", attributes: tagline)
    tag.draw(at: NSPoint(x: (width - tag.size().width) / 2, y: height - 104))

    // Dashed arrow from the app to Applications (icons sit at y = 220 from the top → 200 from the bottom).
    let y = height - 220
    let path = NSBezierPath()
    path.move(to: NSPoint(x: 250, y: y))
    path.line(to: NSPoint(x: 400, y: y))
    path.lineWidth = 3
    path.setLineDash([8, 7], count: 2, phase: 0)
    path.lineCapStyle = .round
    NSColor(red: 0.99, green: 0.81, blue: 0.02, alpha: 0.9).setStroke()
    path.stroke()
    let head = NSBezierPath()
    head.move(to: NSPoint(x: 398, y: y + 10))
    head.line(to: NSPoint(x: 414, y: y))
    head.line(to: NSPoint(x: 398, y: y - 10))
    head.lineWidth = 3
    head.lineCapStyle = .round
    head.lineJoinStyle = .round
    head.stroke()

    let hint: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 13, weight: .medium), .foregroundColor: NSColor(white: 1, alpha: 0.7)]
    let text = NSAttributedString(string: "Drag Agentty into Applications", attributes: hint)
    text.draw(at: NSPoint(x: (width - text.size().width) / 2, y: 58))

    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: out.appendingPathComponent(name))
}

render(scale: 1, name: "background.png")
render(scale: 2, name: "background@2x.png")
print("ok")
