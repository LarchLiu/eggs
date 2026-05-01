import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

struct RGBAImage {
    let width: Int
    let height: Int
    var data: [UInt8]

    init(width: Int, height: Int, data: [UInt8]) {
        self.width = width
        self.height = height
        self.data = data
    }

    init(cgImage: CGImage) {
        width = cgImage.width
        height = cgImage.height
        data = [UInt8](repeating: 0, count: width * height * 4)
        let context = CGContext(
            data: &data,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        )!
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
    }

    func index(x: Int, y: Int) -> Int {
        (y * width + x) * 4
    }

    func pixel(x: Int, y: Int) -> (r: UInt8, g: UInt8, b: UInt8, a: UInt8) {
        let i = index(x: x, y: y)
        return (data[i], data[i + 1], data[i + 2], data[i + 3])
    }

    mutating func setPixel(x: Int, y: Int, r: UInt8, g: UInt8, b: UInt8, a: UInt8) {
        let i = index(x: x, y: y)
        data[i] = r
        data[i + 1] = g
        data[i + 2] = b
        data[i + 3] = a
    }

    func toCGImage() -> CGImage {
        let provider = CGDataProvider(data: Data(data) as CFData)!
        return CGImage(
            width: width,
            height: height,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )!
    }
}

struct Rect: Codable {
    let x: Int
    let y: Int
    let width: Int
    let height: Int
}

struct SourceFrame: Codable {
    let index: Int
    let row: Int
    let column: Int
    let filename: String
    let sourceRect: Rect
    let bounds: Rect?
    let offsetX: Int
    let offsetY: Int
    let anchorX: Double
    let anchorY: Double
}

struct SourceMetadata: Codable {
    let image: String
    let frameWidth: Int
    let frameHeight: Int
    let columns: Int
    let rows: Int
    let frameCount: Int
    let frames: [SourceFrame]
}

struct MergedFrame: Codable {
    let index: Int
    let row: Int
    let column: Int
    let sourceSheet: String
    let sourceIndex: Int
    let bounds: Rect?
    let offsetX: Int
    let offsetY: Int
    let anchorX: Double
    let anchorY: Double
}

struct MergedMetadata: Codable {
    let image: String
    let frameWidth: Int
    let frameHeight: Int
    let columns: Int
    let rows: Int
    let frameCount: Int
    let sources: [String]
    let frames: [MergedFrame]
}

struct InputSheet {
    let metadataPath: String
    let baseDir: URL
    let metadata: SourceMetadata
    let image: RGBAImage
}

func usage() -> String {
    """
    Usage:
      merge_spritesheets <output-dir> <metadata.json> [metadata.json ...]

    Merges already-extracted regular-grid spritesheets vertically into one sheet.
    Sources may have different row counts and frame sizes; frames are centered into
    a common max frame size. All sources must have the same column count.
    """
}

func loadImage(at path: String) throws -> RGBAImage {
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw NSError(domain: "MergeSpritesheets", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unable to load image at \(path)"])
    }
    return RGBAImage(cgImage: image)
}

func savePNG(_ image: RGBAImage, to path: String) throws {
    let url = URL(fileURLWithPath: path)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw NSError(domain: "MergeSpritesheets", code: 2, userInfo: [NSLocalizedDescriptionKey: "Unable to create PNG destination at \(path)"])
    }
    CGImageDestinationAddImage(destination, image.toCGImage(), nil)
    guard CGImageDestinationFinalize(destination) else {
        throw NSError(domain: "MergeSpritesheets", code: 3, userInfo: [NSLocalizedDescriptionKey: "Failed to save PNG at \(path)"])
    }
}

func crop(_ image: RGBAImage, rect: Rect) -> RGBAImage {
    var out = RGBAImage(width: rect.width, height: rect.height, data: [UInt8](repeating: 0, count: rect.width * rect.height * 4))
    for y in 0..<rect.height {
        for x in 0..<rect.width {
            let p = image.pixel(x: rect.x + x, y: rect.y + y)
            out.setPixel(x: x, y: y, r: p.r, g: p.g, b: p.b, a: p.a)
        }
    }
    return out
}

func paste(_ source: RGBAImage, into target: inout RGBAImage, x offsetX: Int, y offsetY: Int) {
    for y in 0..<source.height {
        for x in 0..<source.width {
            let targetX = offsetX + x
            let targetY = offsetY + y
            guard targetX >= 0, targetX < target.width, targetY >= 0, targetY < target.height else {
                continue
            }
            let p = source.pixel(x: x, y: y)
            target.setPixel(x: targetX, y: targetY, r: p.r, g: p.g, b: p.b, a: p.a)
        }
    }
}

func shifted(_ rect: Rect?, x dx: Int, y dy: Int) -> Rect? {
    guard let rect else { return nil }
    return Rect(x: rect.x + dx, y: rect.y + dy, width: rect.width, height: rect.height)
}

let args = CommandLine.arguments
do {
    guard args.count >= 3 else {
        throw NSError(domain: "MergeSpritesheets", code: 4, userInfo: [NSLocalizedDescriptionKey: usage()])
    }

    let outputDir = URL(fileURLWithPath: args[1])
    let metadataPaths = Array(args.dropFirst(2))
    try FileManager.default.createDirectory(at: outputDir, withIntermediateDirectories: true)

    let decoder = JSONDecoder()
    let inputs: [InputSheet] = try metadataPaths.map { path in
        let metadataURL = URL(fileURLWithPath: path)
        let metadata = try decoder.decode(SourceMetadata.self, from: Data(contentsOf: metadataURL))
        let baseDir = metadataURL.deletingLastPathComponent()
        let imagePath = baseDir.appendingPathComponent(metadata.image).path
        return InputSheet(metadataPath: path, baseDir: baseDir, metadata: metadata, image: try loadImage(at: imagePath))
    }

    guard let first = inputs.first else {
        throw NSError(domain: "MergeSpritesheets", code: 5, userInfo: [NSLocalizedDescriptionKey: "No input sheets provided."])
    }
    let columns = first.metadata.columns
    guard inputs.allSatisfy({ $0.metadata.columns == columns }) else {
        throw NSError(domain: "MergeSpritesheets", code: 6, userInfo: [NSLocalizedDescriptionKey: "All sources must have the same column count."])
    }

    let frameWidth = inputs.map(\.metadata.frameWidth).max() ?? 1
    let frameHeight = inputs.map(\.metadata.frameHeight).max() ?? 1
    let rows = inputs.reduce(0) { $0 + $1.metadata.rows }
    var output = RGBAImage(
        width: columns * frameWidth,
        height: rows * frameHeight,
        data: [UInt8](repeating: 0, count: columns * frameWidth * rows * frameHeight * 4)
    )
    var mergedFrames: [MergedFrame] = []
    var rowOffset = 0
    var outputIndex = 0

    for input in inputs {
        let dx = (frameWidth - input.metadata.frameWidth) / 2
        let dy = (frameHeight - input.metadata.frameHeight) / 2
        for sourceFrame in input.metadata.frames.sorted(by: { $0.index < $1.index }) {
            let sourceRect = Rect(
                x: sourceFrame.column * input.metadata.frameWidth,
                y: sourceFrame.row * input.metadata.frameHeight,
                width: input.metadata.frameWidth,
                height: input.metadata.frameHeight
            )
            let frame = crop(input.image, rect: sourceRect)
            let outCol = sourceFrame.column
            let outRow = rowOffset + sourceFrame.row
            paste(frame, into: &output, x: outCol * frameWidth + dx, y: outRow * frameHeight + dy)
            mergedFrames.append(MergedFrame(
                index: outputIndex,
                row: outRow,
                column: outCol,
                sourceSheet: URL(fileURLWithPath: input.metadataPath).lastPathComponent,
                sourceIndex: sourceFrame.index,
                bounds: shifted(sourceFrame.bounds, x: dx, y: dy),
                offsetX: sourceFrame.offsetX + dx,
                offsetY: sourceFrame.offsetY + dy,
                anchorX: (sourceFrame.anchorX * Double(input.metadata.frameWidth) + Double(dx)) / Double(frameWidth),
                anchorY: (sourceFrame.anchorY * Double(input.metadata.frameHeight) + Double(dy)) / Double(frameHeight)
            ))
            outputIndex += 1
        }
        rowOffset += input.metadata.rows
    }

    let imageName = "spritesheet.png"
    let jsonName = "spritesheet.json"
    try savePNG(output, to: outputDir.appendingPathComponent(imageName).path)

    let metadata = MergedMetadata(
        image: imageName,
        frameWidth: frameWidth,
        frameHeight: frameHeight,
        columns: columns,
        rows: rows,
        frameCount: mergedFrames.count,
        sources: metadataPaths.map { URL(fileURLWithPath: $0).lastPathComponent },
        frames: mergedFrames
    )
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(metadata).write(to: outputDir.appendingPathComponent(jsonName))

    print("Merged \(inputs.count) sheets")
    print("Grid: \(columns)x\(rows)")
    print("Frame size: \(frameWidth)x\(frameHeight)")
    print("Sprite sheet: \(outputDir.appendingPathComponent(imageName).path)")
    print("Metadata: \(outputDir.appendingPathComponent(jsonName).path)")
} catch {
    fputs("Error: \(error.localizedDescription)\n", stderr)
    exit(1)
}
