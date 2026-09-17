// Generates Agentty's app icon (macOS rounded-square shape), in-app logo and menu bar template icon
// from the transparent source logo.
//
// Usage: swift scripts/make-icons.swift <transparent-logo.png> <out-dir>
// Produces: <out>/Agentty.iconset/*, <out>/logo-256.png, <out>/logo-mark.png, <out>/menubar.png, <out>/menubar@2x.png

import AppKit
import CoreGraphics
import Foundation

let args = CommandLine.arguments
guard args.count == 3, let source = NSImage(contentsOfFile: args[1]),
      let sourceCG = source.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write("usage: make-icons.swift <logo.png> <out-dir>\n".data(using: .utf8)!)
    exit(1)
}
let outDir = URL(fileURLWithPath: args[2])
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

let yellow = (r: 0xFC, g: 0xCF, b: 0x04)

/// Reads RGBA pixels of the source.
func pixels(_ image: CGImage) -> (data: [UInt8], width: Int, height: Int) {
    let w = image.width, h = image.height
    var data = [UInt8](repeating: 0, count: w * h * 4)
    let ctx = CGContext(data: &data, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
    return (data, w, h)
}

/// Mark only (yellow shape + white prompt), everything else transparent; cropped to its bounds.
func cleanMark() -> CGImage {
    var (data, w, h) = pixels(sourceCG)
    var minX = w, minY = h, maxX = 0, maxY = 0
    for y in 0..<h {
        for x in 0..<w {
            let i = (y * w + x) * 4
            let (r, g, b, a) = (Int(data[i]), Int(data[i + 1]), Int(data[i + 2]), Int(data[i + 3]))
            let isYellow = r > 180 && g > 150 && b < 120 && a > 40
            if isYellow {
                data[i] = UInt8(yellow.r * a / 255); data[i + 1] = UInt8(yellow.g * a / 255); data[i + 2] = UInt8(yellow.b * a / 255)
                minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
            } else if a > 40 && r > 200 && g > 200 && b > 200 {
                // Prompt glyph: keep (white)
                data[i] = UInt8(a); data[i + 1] = UInt8(a); data[i + 2] = UInt8(a)
            } else {
                data[i] = 0; data[i + 1] = 0; data[i + 2] = 0; data[i + 3] = 0
            }
        }
    }
    let ctx = CGContext(data: &data, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let full = ctx.makeImage()!
    // CGImage origin is top-left for cropping; context rows are bottom-up.
    let rect = CGRect(x: minX, y: h - 1 - maxY, width: maxX - minX + 1, height: maxY - minY + 1)
    return full.cropping(to: rect) ?? full
}

let mark = cleanMark()

func save(_ image: CGImage, _ name: String) {
    let rep = NSBitmapImageRep(cgImage: image)
    try! rep.representation(using: .png, properties: [:])!.write(to: outDir.appendingPathComponent(name))
}

func canvas(_ size: Int, _ draw: (CGContext) -> Void) -> CGImage {
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    draw(ctx)
    return ctx.makeImage()!
}

func drawMark(_ ctx: CGContext, in box: CGRect) {
    let scale = min(box.width / CGFloat(mark.width), box.height / CGFloat(mark.height))
    let w = CGFloat(mark.width) * scale, h = CGFloat(mark.height) * scale
    ctx.draw(mark, in: CGRect(x: box.midX - w / 2, y: box.midY - h / 2, width: w, height: h))
}

/// macOS Big Sur+ icon grid: 824pt rounded square centered in 1024, corner radius ~185.
func appIcon(_ size: Int) -> CGImage {
    canvas(size) { ctx in
        let s = CGFloat(size) / 1024
        let tile = CGRect(x: 100 * s, y: 100 * s, width: 824 * s, height: 824 * s)
        let path = CGPath(roundedRect: tile, cornerWidth: 185 * s, cornerHeight: 185 * s, transform: nil)
        // Soft drop shadow like system icons.
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -10 * s), blur: 24 * s, color: CGColor(gray: 0, alpha: 0.35))
        ctx.addPath(path)
        ctx.setFillColor(CGColor(red: 0x19 / 255, green: 0x1a / 255, blue: 0x1f / 255, alpha: 1))
        ctx.fillPath()
        ctx.restoreGState()
        // Subtle top highlight.
        ctx.saveGState()
        ctx.addPath(path)
        ctx.clip()
        let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
                                  colors: [CGColor(gray: 1, alpha: 0.07), CGColor(gray: 1, alpha: 0)] as CFArray, locations: [0, 1])!
        ctx.drawLinearGradient(gradient, start: CGPoint(x: 0, y: tile.maxY), end: CGPoint(x: 0, y: tile.midY), options: [])
        ctx.restoreGState()
        drawMark(ctx, in: tile.insetBy(dx: 150 * s, dy: 150 * s))
    }
}

let iconset = outDir.appendingPathComponent("Agentty.iconset")
try? FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
for (base, name) in [(16, "16x16"), (32, "32x32"), (128, "128x128"), (256, "256x256"), (512, "512x512")] {
    save(appIcon(base), "Agentty.iconset/icon_\(name).png")
    save(appIcon(base * 2), "Agentty.iconset/icon_\(name)@2x.png")
}
save(appIcon(256), "logo-256.png")
// Bare mark on transparent background for the UI (welcome page, mini panel, activity bar).
save(canvas(256) { drawMark($0, in: CGRect(x: 8, y: 8, width: 240, height: 240)) }, "logo-mark.png")

/// Menu bar template: black silhouette of the mark with the prompt cut out.
func menubar(_ size: Int) -> CGImage {
    let colored = canvas(size) { drawMark($0, in: CGRect(x: 0, y: 0, width: size, height: size)) }
    var (data, w, h) = pixels(colored)
    for i in stride(from: 0, to: w * h * 4, by: 4) {
        let (r, g, b, a) = (Int(data[i]), Int(data[i + 1]), Int(data[i + 2]), Int(data[i + 3]))
        let white = r > 200 && g > 200 && b > 200
        let alpha = white ? 0 : a
        data[i] = 0; data[i + 1] = 0; data[i + 2] = 0; data[i + 3] = UInt8(alpha)
    }
    let ctx = CGContext(data: &data, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    return ctx.makeImage()!
}
save(menubar(18), "menubar.png")
save(menubar(36), "menubar@2x.png")
print("ok")
