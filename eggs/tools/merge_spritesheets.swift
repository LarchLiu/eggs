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

struct PetManifest: Encodable {
    let id: String
    let displayName: String
    let description: String
    let spritesheetPath: String
}

enum ImageFormat: String {
    case png
    case webp

    var fileExtension: String {
        rawValue
    }
}

func usage() -> String {
    """
    Usage:
      merge_spritesheets <output-dir> [--format <png|webp>] <metadata.json> [metadata.json ...]

    Merges already-extracted regular-grid spritesheets vertically into one sheet.
    Writes `spritesheet.png`/`.webp` and `metadata.json`.
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

func toolExists(named tool: String) -> Bool {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = ["bash", "-lc", "command -v \(tool) >/dev/null 2>&1"]
    do {
        try process.run()
        process.waitUntilExit()
        return process.terminationStatus == 0
    } catch {
        return false
    }
}

func runCWebP(inputPNG: URL, outputWebP: URL) throws {
    guard toolExists(named: "cwebp") else {
        throw NSError(
            domain: "MergeSpritesheets",
            code: 30,
            userInfo: [NSLocalizedDescriptionKey: "WebP export requires 'cwebp', but it was not found in PATH.\nInstall it with:\n  brew install webp\nThen run the command again, or use '--format png'."]
        )
    }

    let process = Process()
    let errorPipe = Pipe()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = ["cwebp", "-lossless", inputPNG.path, "-o", outputWebP.path]
    process.standardError = errorPipe
    process.standardOutput = Pipe()
    try process.run()
    process.waitUntilExit()

    if process.terminationStatus != 0 {
        let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
        let errorText = String(data: errorData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let detail = (errorText?.isEmpty == false) ? "\n\(errorText!)" : ""
        throw NSError(
            domain: "MergeSpritesheets",
            code: 31,
            userInfo: [NSLocalizedDescriptionKey: "cwebp failed to generate \(outputWebP.lastPathComponent).\(detail)"]
        )
    }
}

func capitalizeFirstLetter(_ text: String) -> String {
    guard let first = text.first else { return text }
    return String(first).uppercased() + text.dropFirst()
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

func loadFrameImage(baseDir: URL, sourceFrame: SourceFrame, fallbackSheet: RGBAImage, frameWidth: Int, frameHeight: Int) throws -> RGBAImage {
    let frameURL = baseDir.appendingPathComponent(sourceFrame.filename)
    if FileManager.default.fileExists(atPath: frameURL.path) {
        return try loadImage(at: frameURL.path)
    }

    let sourceRect = Rect(
        x: sourceFrame.column * frameWidth,
        y: sourceFrame.row * frameHeight,
        width: frameWidth,
        height: frameHeight
    )
    return crop(fallbackSheet, rect: sourceRect)
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
    if args.contains("--help") || args.contains("-h") {
        print(usage())
        exit(0)
    }

    guard args.count >= 3 else {
        throw NSError(domain: "MergeSpritesheets", code: 4, userInfo: [NSLocalizedDescriptionKey: usage()])
    }

    let outputDir = URL(fileURLWithPath: args[1])
    var imageFormat = ImageFormat.png
    var metadataPaths: [String] = []

    var i = 2
    while i < args.count {
        let option = args[i]
        if option == "--format" {
            guard i + 1 < args.count, let parsed = ImageFormat(rawValue: args[i + 1].lowercased()) else {
                throw NSError(domain: "MergeSpritesheets", code: 8, userInfo: [NSLocalizedDescriptionKey: "--format expects 'png' or 'webp'."])
            }
            imageFormat = parsed
            i += 2
        } else {
            metadataPaths.append(option)
            i += 1
        }
    }
    guard !metadataPaths.isEmpty else {
        throw NSError(domain: "MergeSpritesheets", code: 5, userInfo: [NSLocalizedDescriptionKey: "No input sheets provided."])
    }
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
            let frame = try loadFrameImage(
                baseDir: input.baseDir,
                sourceFrame: sourceFrame,
                fallbackSheet: input.image,
                frameWidth: input.metadata.frameWidth,
                frameHeight: input.metadata.frameHeight
            )
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

    let pngImageName = "spritesheet.png"
    let jsonName = "metadata.json"
    let pngImageURL = outputDir.appendingPathComponent(pngImageName)
    let jsonURL = outputDir.appendingPathComponent(jsonName)
    try savePNG(output, to: pngImageURL.path)
    let imageURL: URL
    let imageName: String
    switch imageFormat {
    case .png:
        imageURL = pngImageURL
        imageName = pngImageName
    case .webp:
        let webpName = "spritesheet.webp"
        let webpURL = outputDir.appendingPathComponent(webpName)
        try runCWebP(inputPNG: pngImageURL, outputWebP: webpURL)
        try? FileManager.default.removeItem(at: pngImageURL)
        imageURL = webpURL
        imageName = webpName
    }

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
    try encoder.encode(metadata).write(to: jsonURL)
    let petId = outputDir.lastPathComponent
    let petManifest = PetManifest(
        id: petId,
        displayName: capitalizeFirstLetter(petId),
        description: "",
        spritesheetPath: imageName
    )
    let petJSONURL = outputDir.appendingPathComponent("pet.json")
    try encoder.encode(petManifest).write(to: petJSONURL)

    print("Merged \(inputs.count) sheets")
    print("Grid: \(columns)x\(rows)")
    print("Frame size: \(frameWidth)x\(frameHeight)")
    print("Sprite sheet: \(imageURL.path)")
    print("Metadata: \(jsonURL.path)")
    print("Pet manifest: \(petJSONURL.path)")
} catch {
    fputs("Error: \(error.localizedDescription)\n", stderr)
    exit(1)
}
