// Renders the README banner (docs/assets/banner.png): logo mark, name and tagline on the left, a
// terminal window on the right. Drawn at 2x for sharp rendering on GitHub.
// Usage: swift scripts/make-banner.swift <logo-mark.png> <out.png>
import AppKit

let args = CommandLine.arguments
guard args.count == 3, let mark = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("usage: make-banner.swift <logo-mark.png> <out.png>\n".data(using: .utf8)!)
    exit(1)
}

let width: CGFloat = 1600, height: CGFloat = 533, scale: CGFloat = 2
let yellow = NSColor(red: 0.976, green: 0.784, blue: 0.137, alpha: 1)

/// Top-left based rectangle → AppKit (bottom-left) coordinates.
func rect(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> NSRect {
    NSRect(x: x, y: height - y - h, width: w, height: h)
}

func font(_ names: [String], size: CGFloat, weight: NSFont.Weight) -> NSFont {
    for name in names {
        if let font = NSFont(name: name, size: size) { return font }
    }
    return NSFont.systemFont(ofSize: size, weight: weight)
}

func drawCentered(_ text: String, font: NSFont, color: NSColor, centerX: CGFloat, top: CGFloat, kern: CGFloat = 0) {
    let string = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color, .kern: kern])
    let size = string.size()
    string.draw(at: NSPoint(x: centerX - size.width / 2, y: height - top - size.height))
}

let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(width * scale), pixelsHigh: Int(height * scale),
                           bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: width, height: height)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

// Background.
NSColor(red: 0.075, green: 0.078, blue: 0.086, alpha: 1).setFill()
NSRect(x: 0, y: 0, width: width, height: height).fill()

// Left: mark, name, tagline.
let centerX: CGFloat = 462
mark.draw(in: rect(centerX - 125, 28, 250, 250))
drawCentered("Agentty", font: font(["AvenirNext-Bold", "Avenir-Black"], size: 112, weight: .bold), color: .white,
             centerX: centerX, top: 236, kern: -1)
let tagline = font(["AvenirNext-Regular", "Avenir-Book"], size: 34, weight: .regular)
let gray = NSColor(white: 0.66, alpha: 1)
drawCentered("The orchestration terminal", font: tagline, color: gray, centerX: centerX, top: 404, kern: 0.5)
drawCentered("for AI-native workflows.", font: tagline, color: gray, centerX: centerX, top: 447, kern: 0.5)

// Right: terminal window.
let window = rect(958, 77, 562, 384)
let frame = NSBezierPath(roundedRect: window, xRadius: 16, yRadius: 16)
NSColor(red: 0.082, green: 0.086, blue: 0.094, alpha: 1).setFill()
frame.fill()
NSGraphicsContext.saveGraphicsState()
frame.addClip()
NSColor(red: 0.11, green: 0.114, blue: 0.125, alpha: 1).setFill()
rect(958, 77, 562, 44).fill()
NSColor(white: 1, alpha: 0.06).setFill()
rect(958, 121, 562, 1.5).fill()
NSGraphicsContext.restoreGraphicsState()
NSColor(red: 0.29, green: 0.3, blue: 0.32, alpha: 1).setStroke()
frame.lineWidth = 3
frame.stroke()
for (index, color) in [NSColor(red: 0.93, green: 0.33, blue: 0.3, alpha: 1), NSColor(red: 0.96, green: 0.74, blue: 0.2, alpha: 1),
                       NSColor(red: 0.3, green: 0.76, blue: 0.3, alpha: 1)].enumerated() {
    color.setFill()
    NSBezierPath(ovalIn: rect(979 + CGFloat(index) * 29, 92, 16, 16)).fill()
}

// Prompt `>_`.
let chevron = NSBezierPath()
chevron.move(to: NSPoint(x: 1014, y: height - 166))
chevron.line(to: NSPoint(x: 1060, y: height - 197))
chevron.line(to: NSPoint(x: 1014, y: height - 228))
chevron.lineWidth = 17
chevron.lineCapStyle = .round
chevron.lineJoinStyle = .round
yellow.setStroke()
chevron.stroke()
yellow.setFill()
NSBezierPath(roundedRect: rect(1072, 221, 66, 18), xRadius: 6, yRadius: 6).fill()

// Output lines.
NSColor(red: 0.29, green: 0.3, blue: 0.32, alpha: 1).setFill()
for (top, length) in [(270, 350), (311, 287), (350, 420), (391, 208)] as [(CGFloat, CGFloat)] {
    NSBezierPath(roundedRect: rect(1007, top, length, 19), xRadius: 5, yRadius: 5).fill()
}

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
