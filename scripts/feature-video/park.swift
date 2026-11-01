// park <x> <y>: moves the pointer to a screen point (points, top-left origin), out of the recorded window.
import CoreGraphics
import Foundation

let a = CommandLine.arguments
guard a.count == 3, let x = Double(a[1]), let y = Double(a[2]) else { print("usage: park <x> <y>"); exit(2) }
CGWarpMouseCursorPosition(CGPoint(x: x, y: y))
