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

struct ToolOptions {
    let inputDir: URL
    let frameWidth: Int
    let frameHeight: Int
    let columns: Int
    let outputName: String?
    let imageFormat: ImageFormat
}

enum ImageFormat: String {
    case png
    case webp

    var fileName: String {
        "spritesheet.\(rawValue)"
    }

    var displayName: String {
        switch self {
        case .png:
            return "PNG"
        case .webp:
            return "WebP"
        }
    }
}

struct MetadataFrame: Decodable {
    let row: Int
    let column: Int
    let filename: String
}

struct MetadataSheet: Decodable {
    let columns: Int
    let rows: Int
    let frames: [MetadataFrame]
}

struct PositionedFrame {
    let url: URL
    let row: Int
    let column: Int
}

struct OutputFrameMetadata: Encodable {
    let index: Int
    let row: Int
    let column: Int
    let filename: String
}

struct OutputMetadata: Encodable {
    let image: String
    let frameWidth: Int
    let frameHeight: Int
    let columns: Int
    let rows: Int
    let sourceColumns: Int
    let sourceRows: Int
    let frameCount: Int
    let frames: [OutputFrameMetadata]
}

struct PetManifest: Encodable {
    let id: String
    let displayName: String
    let description: String
    let spritesheetPath: String
}

func detectFramePrefix(from filename: String) -> String? {
    let stem = URL(fileURLWithPath: filename).deletingPathExtension().lastPathComponent
    if let range = stem.range(of: #"_(\d+)x(\d+)$"#, options: .regularExpression) {
        return String(stem[..<range.lowerBound])
    }
    if let range = stem.range(of: #"_(\d+)$"#, options: .regularExpression) {
        return String(stem[..<range.lowerBound])
    }
    return nil
}

func frameFilename(prefix: String, row: Int, column: Int, rowDigits: Int, columnDigits: Int) -> String {
    String(
        format: "%@_%0*d x %0*d.png".replacingOccurrences(of: " ", with: ""),
        prefix,
        rowDigits,
        row,
        columnDigits,
        column
    )
}

func usage() -> String {
    """
    Usage:
      resize_crop_frames <frames-dir> <width>x<height>|<size> [--x <columns>] [--out <name>] [--format <png|webp>]

    Resizes each PNG frame by scaling the longest source edge to the target's
    longest edge, then center-crops into the target canvas. Processed frames plus
    a pet package (`pet.json` + `spritesheet`) are written to a sibling output dir.

    Options:
      --x <columns>   Number of columns in the output spritesheet. Default: 8.
      --out <name>    Output directory name. Default: frames-<width>x<height>.
      --format <fmt>  Spritesheet format: png or webp. Default: png.
      --help          Show this help.
    """
}

func parsePositiveInt(_ value: String, option: String) throws -> Int {
    guard let number = Int(value), number > 0 else {
        throw NSError(domain: "ResizeCropFrames", code: 10, userInfo: [NSLocalizedDescriptionKey: "\(option) expects a positive integer, got '\(value)'."])
    }
    return number
}

func parseFrameSize(_ value: String) throws -> (width: Int, height: Int) {
    let parts = value.lowercased().split(separator: "x").map(String.init)
    if parts.count == 1 {
        let size = try parsePositiveInt(parts[0], option: "size")
        return (size, size)
    }
    guard parts.count == 2 else {
        throw NSError(domain: "ResizeCropFrames", code: 11, userInfo: [NSLocalizedDescriptionKey: "Size expects '<width>x<height>' or a single square size."])
    }
    return (
        try parsePositiveInt(parts[0], option: "width"),
        try parsePositiveInt(parts[1], option: "height")
    )
}

func parseOptions(_ args: [String]) throws -> ToolOptions {
    if args.contains("--help") || args.contains("-h") {
        print(usage())
        exit(0)
    }

    guard args.count >= 3 else {
        throw NSError(domain: "ResizeCropFrames", code: 12, userInfo: [NSLocalizedDescriptionKey: usage()])
    }

    let inputDir = URL(fileURLWithPath: args[1]).standardizedFileURL
    let size = try parseFrameSize(args[2])
    var columns = 8
    var outputName: String? = nil
    var imageFormat: ImageFormat = .png

    var index = 3
    while index < args.count {
        let option = args[index]
        func requireValue() throws -> String {
            guard index + 1 < args.count else {
                throw NSError(domain: "ResizeCropFrames", code: 13, userInfo: [NSLocalizedDescriptionKey: "\(option) requires a value."])
            }
            return args[index + 1]
        }

        switch option {
        case "--x", "--columns":
            columns = try parsePositiveInt(try requireValue(), option: option)
            index += 2
        case "--out":
            outputName = try requireValue()
            index += 2
        case "--format":
            let value = try requireValue()
            guard let parsed = ImageFormat(rawValue: value.lowercased()) else {
                throw NSError(domain: "ResizeCropFrames", code: 15, userInfo: [NSLocalizedDescriptionKey: "--format expects 'png' or 'webp'."])
            }
            imageFormat = parsed
            index += 2
        default:
            throw NSError(domain: "ResizeCropFrames", code: 14, userInfo: [NSLocalizedDescriptionKey: "Unknown option '\(option)'.\n\(usage())"])
        }
    }

    return ToolOptions(
        inputDir: inputDir,
        frameWidth: size.width,
        frameHeight: size.height,
        columns: columns,
        outputName: outputName,
        imageFormat: imageFormat
    )
}

func loadImage(at path: String) throws -> RGBAImage {
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw NSError(domain: "ResizeCropFrames", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unable to load image at \(path)"])
    }
    return RGBAImage(cgImage: image)
}

func saveImage(_ image: RGBAImage, to path: String, format: ImageFormat) throws {
    guard format == .png else {
        throw NSError(domain: "ResizeCropFrames", code: 2, userInfo: [NSLocalizedDescriptionKey: "saveImage currently only supports PNG output directly."])
    }
    let url = URL(fileURLWithPath: path)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw NSError(domain: "ResizeCropFrames", code: 2, userInfo: [NSLocalizedDescriptionKey: "Unable to create image destination at \(path)"])
    }
    CGImageDestinationAddImage(destination, image.toCGImage(), nil)
    guard CGImageDestinationFinalize(destination) else {
        throw NSError(domain: "ResizeCropFrames", code: 3, userInfo: [NSLocalizedDescriptionKey: "Failed to save image at \(path)"])
    }
}

func resizeImage(_ image: RGBAImage, width: Int, height: Int) -> RGBAImage {
    var data = [UInt8](repeating: 0, count: width * height * 4)
    let context = CGContext(
        data: &data,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: width * 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
    )!
    context.interpolationQuality = .none
    context.clear(CGRect(x: 0, y: 0, width: width, height: height))
    context.draw(image.toCGImage(), in: CGRect(x: 0, y: 0, width: width, height: height))
    return RGBAImage(width: width, height: height, data: data)
}

func scaleAndCenterCrop(_ image: RGBAImage, targetWidth: Int, targetHeight: Int) -> RGBAImage {
    let sourceLongestEdge = max(image.width, image.height)
    let targetLongestEdge = max(targetWidth, targetHeight)
    let scale = min(1.0, Double(targetLongestEdge) / Double(sourceLongestEdge))
    let scaledWidth = max(1, Int(round(Double(image.width) * scale)))
    let scaledHeight = max(1, Int(round(Double(image.height) * scale)))
    let scaled = (scaledWidth == image.width && scaledHeight == image.height)
        ? image
        : resizeImage(image, width: scaledWidth, height: scaledHeight)

    var output = RGBAImage(
        width: targetWidth,
        height: targetHeight,
        data: [UInt8](repeating: 0, count: targetWidth * targetHeight * 4)
    )

    let sourceStartX = max(0, (scaled.width - targetWidth) / 2)
    let sourceStartY = max(0, (scaled.height - targetHeight) / 2)
    let destStartX = max(0, (targetWidth - scaled.width) / 2)
    let destStartY = max(0, (targetHeight - scaled.height) / 2)
    let copyWidth = min(targetWidth, scaled.width)
    let copyHeight = min(targetHeight, scaled.height)

    for y in 0..<copyHeight {
        for x in 0..<copyWidth {
            let pixel = scaled.pixel(x: sourceStartX + x, y: sourceStartY + y)
            output.setPixel(x: destStartX + x, y: destStartY + y, r: pixel.r, g: pixel.g, b: pixel.b, a: pixel.a)
        }
    }

    return output
}

func combineFrames(_ frames: [RGBAImage], columns: Int, frameWidth: Int, frameHeight: Int) -> RGBAImage {
    guard !frames.isEmpty else {
        return RGBAImage(width: frameWidth, height: frameHeight, data: [UInt8](repeating: 0, count: frameWidth * frameHeight * 4))
    }

    let rows = Int(ceil(Double(frames.count) / Double(columns)))
    var sheet = RGBAImage(
        width: frameWidth * columns,
        height: frameHeight * rows,
        data: [UInt8](repeating: 0, count: frameWidth * columns * frameHeight * rows * 4)
    )

    for (index, frame) in frames.enumerated() {
        let column = index % columns
        let row = index / columns
        let offsetX = column * frameWidth
        let offsetY = row * frameHeight
        for y in 0..<frame.height {
            for x in 0..<frame.width {
                let pixel = frame.pixel(x: x, y: y)
                sheet.setPixel(x: offsetX + x, y: offsetY + y, r: pixel.r, g: pixel.g, b: pixel.b, a: pixel.a)
            }
        }
    }

    return sheet
}

func combinePositionedFrames(
    _ frames: [(position: PositionedFrame, image: RGBAImage)],
    columns: Int,
    rows: Int,
    frameWidth: Int,
    frameHeight: Int
) -> RGBAImage {
    var sheet = RGBAImage(
        width: frameWidth * columns,
        height: frameHeight * rows,
        data: [UInt8](repeating: 0, count: frameWidth * columns * frameHeight * rows * 4)
    )

    for item in frames {
        let offsetX = item.position.column * frameWidth
        let offsetY = item.position.row * frameHeight
        for y in 0..<item.image.height {
            for x in 0..<item.image.width {
                let pixel = item.image.pixel(x: x, y: y)
                sheet.setPixel(x: offsetX + x, y: offsetY + y, r: pixel.r, g: pixel.g, b: pixel.b, a: pixel.a)
            }
        }
    }

    return sheet
}

func removeExistingPNGs(in directory: URL) throws {
    guard FileManager.default.fileExists(atPath: directory.path) else {
        return
    }
    let contents = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
    for item in contents where ["png", "webp"].contains(item.pathExtension.lowercased()) {
        try FileManager.default.removeItem(at: item)
    }
}

func filenamePrefix(from inputDir: URL, frameFiles: [URL]) -> String {
    for frameURL in frameFiles {
        if let prefix = detectFramePrefix(from: frameURL.lastPathComponent), !prefix.isEmpty {
            return prefix
        }
    }
    let name = inputDir.lastPathComponent
    if name == "frames" {
        return inputDir.deletingLastPathComponent().lastPathComponent
    }
    if let range = name.range(of: "-") {
        return String(name[..<range.lowerBound])
    }
    return name
}

func capitalizeFirstLetter(_ text: String) -> String {
    guard let first = text.first else { return text }
    return String(first).uppercased() + text.dropFirst()
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
            domain: "ResizeCropFrames",
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
            domain: "ResizeCropFrames",
            code: 31,
            userInfo: [NSLocalizedDescriptionKey: "cwebp failed to generate \(outputWebP.lastPathComponent).\(detail)"]
        )
    }
}

func parseFrameCoordinates(from filename: String) -> (row: Int, column: Int)? {
    let stem = URL(fileURLWithPath: filename).deletingPathExtension().lastPathComponent
    guard let match = stem.range(of: #"(\d+)x(\d+)$"#, options: .regularExpression) else {
        return nil
    }
    let suffix = String(stem[match])
    let parts = suffix.split(separator: "x")
    guard parts.count == 2,
          let row = Int(parts[0]),
          let column = Int(parts[1]) else {
        return nil
    }
    return (row, column)
}

func resolvePositionedFrames(frameFiles: [URL], inputDir: URL) throws -> (frames: [PositionedFrame], sourceColumns: Int?, sourceRows: Int?) {
    let parsed = frameFiles.compactMap { frameURL -> PositionedFrame? in
        guard let coordinates = parseFrameCoordinates(from: frameURL.lastPathComponent) else {
            return nil
        }
        return PositionedFrame(url: frameURL, row: coordinates.row, column: coordinates.column)
    }
    if parsed.count == frameFiles.count {
        let sourceColumns = (parsed.map(\.column).max() ?? -1) + 1
        let sourceRows = (parsed.map(\.row).max() ?? -1) + 1
        return (
            parsed.sorted {
                if $0.row == $1.row { return $0.column < $1.column }
                return $0.row < $1.row
            },
            sourceColumns,
            sourceRows
        )
    }

    let parentDir = inputDir.deletingLastPathComponent()
    let metadataFiles = try FileManager.default.contentsOfDirectory(at: parentDir, includingPropertiesForKeys: nil)
        .filter { $0.pathExtension.lowercased() == "json" }
        .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
    let decoder = JSONDecoder()
    let relativePrefix = inputDir.lastPathComponent + "/"

    for metadataURL in metadataFiles {
        guard let metadata = try? decoder.decode(MetadataSheet.self, from: Data(contentsOf: metadataURL)) else {
            continue
        }
        let byName = Dictionary(uniqueKeysWithValues: metadata.frames.map { ($0.filename, $0) })
        let positioned = frameFiles.compactMap { frameURL -> PositionedFrame? in
            let exactKey = relativePrefix + frameURL.lastPathComponent
            if let frame = byName[exactKey] {
                return PositionedFrame(url: frameURL, row: frame.row, column: frame.column)
            }
            if let frame = byName.values.first(where: { URL(fileURLWithPath: $0.filename).lastPathComponent == frameURL.lastPathComponent }) {
                return PositionedFrame(url: frameURL, row: frame.row, column: frame.column)
            }
            return nil
        }
        if positioned.count == frameFiles.count {
            return (
                positioned.sorted {
                    if $0.row == $1.row { return $0.column < $1.column }
                    return $0.row < $1.row
                },
                metadata.columns,
                metadata.rows
            )
        }
    }

    let fallback = frameFiles.enumerated().map { index, frameURL in
        PositionedFrame(url: frameURL, row: index / 8, column: index % 8)
    }
    return (fallback, nil, nil)
}

let args = CommandLine.arguments
do {
    let options = try parseOptions(args)
    var isDirectory: ObjCBool = false
    guard FileManager.default.fileExists(atPath: options.inputDir.path, isDirectory: &isDirectory), isDirectory.boolValue else {
        throw NSError(domain: "ResizeCropFrames", code: 15, userInfo: [NSLocalizedDescriptionKey: "Input directory does not exist: \(options.inputDir.path)"])
    }

    let frameFiles = try FileManager.default.contentsOfDirectory(at: options.inputDir, includingPropertiesForKeys: nil)
        .filter { $0.pathExtension.lowercased() == "png" }
        .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }

    guard !frameFiles.isEmpty else {
        throw NSError(domain: "ResizeCropFrames", code: 16, userInfo: [NSLocalizedDescriptionKey: "No PNG frames found in \(options.inputDir.path)"])
    }

    let resolved = try resolvePositionedFrames(frameFiles: frameFiles, inputDir: options.inputDir)
    let positionedFrames = resolved.frames
    let sourceColumns = resolved.sourceColumns ?? ((positionedFrames.map(\.column).max() ?? -1) + 1)
    let sourceRows = resolved.sourceRows ?? ((positionedFrames.map(\.row).max() ?? -1) + 1)
    if options.columns < sourceColumns {
        throw NSError(
            domain: "ResizeCropFrames",
            code: 18,
            userInfo: [NSLocalizedDescriptionKey: "Requested --x \(options.columns) is smaller than the source column count \(sourceColumns). Use a value >= \(sourceColumns) to preserve row layout."]
        )
    }

    let parentDir = options.inputDir.deletingLastPathComponent()
    let outputDirName = options.outputName ?? "frames-\(options.frameWidth)x\(options.frameHeight)"
    let outputDir = parentDir.appendingPathComponent(outputDirName, isDirectory: true)
    guard outputDir.standardizedFileURL.path != options.inputDir.path else {
        throw NSError(domain: "ResizeCropFrames", code: 17, userInfo: [NSLocalizedDescriptionKey: "Output directory would overwrite the input directory."])
    }

    try FileManager.default.createDirectory(at: outputDir, withIntermediateDirectories: true)
    try removeExistingPNGs(in: outputDir)

    let prefix = options.outputName ?? filenamePrefix(from: options.inputDir, frameFiles: frameFiles)
    let rowDigits = max(2, String(max(0, sourceRows - 1)).count)
    let columnDigits = max(2, String(max(0, options.columns - 1)).count)
    var processedFrames: [(position: PositionedFrame, image: RGBAImage)] = []
    var outputMetadataFrames: [OutputFrameMetadata] = []
    for frame in positionedFrames {
        let source = try loadImage(at: frame.url.path)
        let processed = scaleAndCenterCrop(source, targetWidth: options.frameWidth, targetHeight: options.frameHeight)
        let outputFilename = frameFilename(
            prefix: prefix,
            row: frame.row,
            column: frame.column,
            rowDigits: rowDigits,
            columnDigits: columnDigits
        )
        let outputURL = outputDir.appendingPathComponent(outputFilename)
        try saveImage(processed, to: outputURL.path, format: .png)
        processedFrames.append((frame, processed))
        outputMetadataFrames.append(OutputFrameMetadata(
            index: frame.row * options.columns + frame.column,
            row: frame.row,
            column: frame.column,
            filename: outputFilename
        ))
    }

    let sheet = combinePositionedFrames(
        processedFrames,
        columns: options.columns,
        rows: sourceRows,
        frameWidth: options.frameWidth,
        frameHeight: options.frameHeight
    )
    let pngSheetURL = outputDir.appendingPathComponent(ImageFormat.png.fileName)
    try saveImage(sheet, to: pngSheetURL.path, format: .png)
    let sheetURL: URL
    switch options.imageFormat {
    case .png:
        sheetURL = pngSheetURL
    case .webp:
        let webpURL = outputDir.appendingPathComponent(ImageFormat.webp.fileName)
        try runCWebP(inputPNG: pngSheetURL, outputWebP: webpURL)
        try? FileManager.default.removeItem(at: pngSheetURL)
        sheetURL = webpURL
    }

    let petId = outputDir.lastPathComponent
    let manifest = PetManifest(
        id: petId,
        displayName: capitalizeFirstLetter(petId),
        description: "",
        spritesheetPath: options.imageFormat.fileName
    )
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    let metadata = OutputMetadata(
        image: options.imageFormat.fileName,
        frameWidth: options.frameWidth,
        frameHeight: options.frameHeight,
        columns: options.columns,
        rows: sourceRows,
        sourceColumns: sourceColumns,
        sourceRows: sourceRows,
        frameCount: outputMetadataFrames.count,
        frames: outputMetadataFrames
    )
    let metadataURL = outputDir.appendingPathComponent("metadata.json")
    try encoder.encode(metadata).write(to: metadataURL)
    let manifestURL = outputDir.appendingPathComponent("pet.json")
    try encoder.encode(manifest).write(to: manifestURL)

    print("Processed \(processedFrames.count) frames")
    print("Frames dir: \(outputDir.path)")
    print("Sheet: \(sheetURL.path)")
    print("Metadata: \(metadataURL.path)")
    print("Manifest: \(manifestURL.path)")
    print("Frame size: \(options.frameWidth)x\(options.frameHeight)")
    print("Source grid: \(sourceColumns)x\(sourceRows)")
    print("Sheet grid: \(options.columns)x\(sourceRows)")
} catch {
    fputs("Error: \(error.localizedDescription)\n", stderr)
    exit(1)
}
