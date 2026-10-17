// Reads the text in every PNG of a folder with Vision. One line per word:
// "<file>\t<x>\t<y>\t<w>\t<h>\t<word>\t<whole line>", the box in pixels from the top-left corner.
import AppKit
import Vision

let folder = URL(fileURLWithPath: CommandLine.arguments[1])
let files = (try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? []
for file in files.filter({ $0.pathExtension == "png" }).sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
    guard let image = NSImage(contentsOf: file)?.cgImage(forProposedRect: nil, context: nil, hints: nil) else { continue }
    let (width, height) = (Double(image.width), Double(image.height))
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    request.recognitionLanguages = ["en-US", "ko-KR", "ja-JP", "zh-Hans"]
    try? VNImageRequestHandler(cgImage: image).perform([request])
    for observation in request.results ?? [] {
        guard let candidate = observation.topCandidates(1).first else { continue }
        let line = candidate.string
        var start = line.startIndex
        // Words split on spaces, each with its own box, so only the private part gets blurred.
        for word in line.split(separator: " ", omittingEmptySubsequences: true) {
            guard let range = line.range(of: word, range: start..<line.endIndex) else { continue }
            start = range.upperBound
            let box = (try? candidate.boundingBox(for: range))?.boundingBox ?? observation.boundingBox
            let x = box.minX * width, y = (1 - box.maxY) * height
            print("\(file.lastPathComponent)\t\(Int(x))\t\(Int(y))\t\(Int(box.width * width))\t\(Int(box.height * height))\t\(word)\t\(line)")
        }
    }
}
