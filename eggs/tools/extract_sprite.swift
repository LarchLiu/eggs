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
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        let bitmapInfo = CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        let context = CGContext(
            data: &data,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: bitmapInfo
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
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        return CGImage(
            width: width,
            height: height,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: width * 4,
            space: colorSpace,
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

struct FrameMetadata: Codable {
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

struct SpriteSheetMetadata: Codable {
    let image: String
    let frameWidth: Int
    let frameHeight: Int
    let columns: Int
    let rows: Int
    let frameCount: Int
    let frames: [FrameMetadata]
}

struct LineGroup {
    let start: Int
    let end: Int
    let score: Double
}

struct FrameCell {
    let cellRect: Rect
    let contentRect: Rect
}

enum AlignmentMode: String {
    case preserveCell = "preserve-cell"
    case centerContent = "center-content"
}

enum GridMode: String {
    case auto
    case bordered
    case uniform
}

struct ToolOptions {
    let inputPath: String
    let outputDir: String
    let spriteName: String
    let columns: Int?
    let rows: Int?
    let frameWidth: Int?
    let frameHeight: Int?
    let prefix: String
    let borderThreshold: Double
    let trimPadding: Int
    let alignment: AlignmentMode
    let gridMode: GridMode
}

func usage() -> String {
    """
    Usage:
      extract_sprite <input.png> <output-dir> [options]

    Options:
      --columns <n>              Override detected column count.
      --rows <n>                 Override detected row count.
      --frame-size <w>x<h>|<n>   Override output frame canvas size.
      --name <name>              Output sprite name. Writes <name>.png and <name>.json.
                                 Defaults to <input-name>_spritesheet.
      --prefix <name>            Individual frame filename prefix. Defaults to output directory name.
      --border-threshold <n>     Border detection threshold, 0.0-1.0. Default: 0.75.
      --padding <px>             Pixels to trim inward from detected cell borders. Default: 5.
      --align <mode>             preserve-cell or center-content. Default: preserve-cell.
      --grid <mode>              auto, bordered, or uniform. Default: auto.
      --help                     Show this help.
    """
}

func parsePositiveInt(_ value: String, option: String) throws -> Int {
    guard let number = Int(value), number > 0 else {
        throw NSError(domain: "SpriteExtract", code: 10, userInfo: [NSLocalizedDescriptionKey: "\(option) expects a positive integer, got '\(value)'."])
    }
    return number
}

func parseFrameSize(_ value: String) throws -> (width: Int, height: Int) {
    let parts = value.lowercased().split(separator: "x").map(String.init)
    if parts.count == 1 {
        let size = try parsePositiveInt(parts[0], option: "--frame-size")
        return (size, size)
    }
    guard parts.count == 2 else {
        throw NSError(domain: "SpriteExtract", code: 11, userInfo: [NSLocalizedDescriptionKey: "--frame-size expects '<width>x<height>' or a single square size."])
    }
    return (
        try parsePositiveInt(parts[0], option: "--frame-size width"),
        try parsePositiveInt(parts[1], option: "--frame-size height")
    )
}

func parseOptions(_ args: [String]) throws -> ToolOptions {
    if args.contains("--help") || args.contains("-h") {
        print(usage())
        exit(0)
    }

    guard args.count >= 3 else {
        throw NSError(domain: "SpriteExtract", code: 12, userInfo: [NSLocalizedDescriptionKey: usage()])
    }

    let inputPath = args[1]
    let outputDir = args[2]
    var columns: Int? = nil
    var rows: Int? = nil
    var frameWidth: Int? = nil
    var frameHeight: Int? = nil
    var spriteName: String? = nil
    var prefix: String? = nil
    var borderThreshold = 0.75
    var trimPadding = 5
    var alignment = AlignmentMode.preserveCell
    var gridMode = GridMode.auto

    var i = 3
    while i < args.count {
        let option = args[i]
        func requireValue() throws -> String {
            guard i + 1 < args.count else {
                throw NSError(domain: "SpriteExtract", code: 13, userInfo: [NSLocalizedDescriptionKey: "\(option) requires a value."])
            }
            return args[i + 1]
        }

        switch option {
        case "--columns":
            columns = try parsePositiveInt(try requireValue(), option: option)
            i += 2
        case "--rows":
            rows = try parsePositiveInt(try requireValue(), option: option)
            i += 2
        case "--frame-size":
            let size = try parseFrameSize(try requireValue())
            frameWidth = size.width
            frameHeight = size.height
            i += 2
        case "--name":
            spriteName = try requireValue()
            i += 2
        case "--prefix":
            prefix = try requireValue()
            i += 2
        case "--border-threshold":
            let value = try requireValue()
            guard let parsed = Double(value), parsed > 0, parsed <= 1 else {
                throw NSError(domain: "SpriteExtract", code: 14, userInfo: [NSLocalizedDescriptionKey: "--border-threshold expects a number in (0, 1]."])
            }
            borderThreshold = parsed
            i += 2
        case "--padding":
            trimPadding = try parsePositiveInt(try requireValue(), option: option)
            i += 2
        case "--align":
            let value = try requireValue()
            guard let parsed = AlignmentMode(rawValue: value) else {
                throw NSError(domain: "SpriteExtract", code: 17, userInfo: [NSLocalizedDescriptionKey: "--align expects 'preserve-cell' or 'center-content'."])
            }
            alignment = parsed
            i += 2
        case "--grid":
            let value = try requireValue()
            guard let parsed = GridMode(rawValue: value) else {
                throw NSError(domain: "SpriteExtract", code: 18, userInfo: [NSLocalizedDescriptionKey: "--grid expects 'auto', 'bordered', or 'uniform'."])
            }
            gridMode = parsed
            i += 2
        default:
            throw NSError(domain: "SpriteExtract", code: 15, userInfo: [NSLocalizedDescriptionKey: "Unknown option '\(option)'.\n\(usage())"])
        }
    }

    let inputStem = URL(fileURLWithPath: inputPath).deletingPathExtension().lastPathComponent
    let defaultSpriteName = inputStem.isEmpty ? "spritesheet" : "\(inputStem)_spritesheet"
    let defaultPrefix = URL(fileURLWithPath: outputDir).lastPathComponent.isEmpty
        ? "sprites"
        : URL(fileURLWithPath: outputDir).lastPathComponent

    return ToolOptions(
        inputPath: inputPath,
        outputDir: outputDir,
        spriteName: spriteName ?? defaultSpriteName,
        columns: columns,
        rows: rows,
        frameWidth: frameWidth,
        frameHeight: frameHeight,
        prefix: prefix ?? defaultPrefix,
        borderThreshold: borderThreshold,
        trimPadding: trimPadding,
        alignment: alignment,
        gridMode: gridMode
    )
}

func loadImage(at path: String) throws -> RGBAImage {
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw NSError(domain: "SpriteExtract", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unable to load image at \(path)"])
    }
    return RGBAImage(cgImage: image)
}

func savePNG(_ image: RGBAImage, to path: String) throws {
    let url = URL(fileURLWithPath: path)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw NSError(domain: "SpriteExtract", code: 2, userInfo: [NSLocalizedDescriptionKey: "Unable to create PNG destination at \(path)"])
    }
    let props: [CFString: Any] = [
        kCGImagePropertyPNGDictionary: [kCGImagePropertyPNGInterlaceType: 0]
    ]
    CGImageDestinationAddImage(destination, image.toCGImage(), props as CFDictionary)
    guard CGImageDestinationFinalize(destination) else {
        throw NSError(domain: "SpriteExtract", code: 3, userInfo: [NSLocalizedDescriptionKey: "Failed to save PNG at \(path)"])
    }
}

func copyReplacingItem(from source: URL, to destination: URL) throws {
    let sourcePath = source.standardizedFileURL.path
    let destinationPath = destination.standardizedFileURL.path
    guard sourcePath != destinationPath else { return }
    if FileManager.default.fileExists(atPath: destination.path) {
        try FileManager.default.removeItem(at: destination)
    }
    try FileManager.default.copyItem(at: source, to: destination)
}

func installSpriteAssets(imageURL: URL, metadataURL: URL) throws -> URL {
    let installDir = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".codex")
        .appendingPathComponent("eggs")
    try FileManager.default.createDirectory(at: installDir, withIntermediateDirectories: true)
    try copyReplacingItem(from: imageURL, to: installDir.appendingPathComponent(imageURL.lastPathComponent))
    try copyReplacingItem(from: metadataURL, to: installDir.appendingPathComponent(metadataURL.lastPathComponent))
    return installDir
}

func darkPixelRatioForColumn(_ image: RGBAImage, x: Int) -> Double {
    var dark = 0
    for y in 0..<image.height {
        let p = image.pixel(x: x, y: y)
        if p.a > 220 && Int(p.r) < 80 && Int(p.g) < 80 && Int(p.b) < 80 {
            dark += 1
        }
    }
    return Double(dark) / Double(image.height)
}

func darkPixelRatioForRow(_ image: RGBAImage, y: Int) -> Double {
    var dark = 0
    for x in 0..<image.width {
        let p = image.pixel(x: x, y: y)
        if p.a > 220 && Int(p.r) < 80 && Int(p.g) < 80 && Int(p.b) < 80 {
            dark += 1
        }
    }
    return Double(dark) / Double(image.width)
}

func bestLineGroup(in range: ClosedRange<Int>, ratioAt: (Int) -> Double) -> LineGroup? {
    let threshold = 0.45
    var groups: [LineGroup] = []
    var start: Int? = nil
    var score = 0.0

    for i in range {
        let ratio = ratioAt(i)
        if ratio >= threshold {
            if start == nil {
                start = i
                score = 0
            }
            score += ratio
        } else if let s = start {
            groups.append(LineGroup(start: s, end: i - 1, score: score))
            start = nil
            score = 0
        }
    }

    if let s = start {
        groups.append(LineGroup(start: s, end: range.upperBound, score: score))
    }

    return groups.max { $0.score < $1.score }
}

func lineGroups(in range: Range<Int>, threshold: Double, ratioAt: (Int) -> Double) -> [LineGroup] {
    var groups: [LineGroup] = []
    var start: Int? = nil
    var score = 0.0

    for i in range {
        let ratio = ratioAt(i)
        if ratio >= threshold {
            if start == nil {
                start = i
                score = 0
            }
            score += ratio
        } else if let s = start {
            groups.append(LineGroup(start: s, end: i - 1, score: score))
            start = nil
            score = 0
        }
    }

    if let s = start {
        groups.append(LineGroup(start: s, end: range.upperBound - 1, score: score))
    }

    return groups
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

func cropInsideCellFrame(_ tile: RGBAImage) -> RGBAImage {
    let marginX = min(70, max(24, tile.width / 3))
    let marginY = min(70, max(24, tile.height / 3))
    let leftGroup = bestLineGroup(in: 0...marginX) { darkPixelRatioForColumn(tile, x: $0) }
    let rightGroup = bestLineGroup(in: (tile.width - marginX - 1)...(tile.width - 1)) { darkPixelRatioForColumn(tile, x: $0) }
    let topGroup = bestLineGroup(in: 0...marginY) { darkPixelRatioForRow(tile, y: $0) }
    let bottomGroup = bestLineGroup(in: (tile.height - marginY - 1)...(tile.height - 1)) { darkPixelRatioForRow(tile, y: $0) }

    let inwardPadding = 4
    let left = min(tile.width - 2, (leftGroup?.end ?? 18) + inwardPadding)
    let right = max(left + 1, (rightGroup?.start ?? (tile.width - 19)) - inwardPadding)
    let top = min(tile.height - 2, (topGroup?.end ?? 18) + inwardPadding)
    let bottom = max(top + 1, (bottomGroup?.start ?? (tile.height - 19)) - inwardPadding)

    return crop(tile, rect: Rect(x: left, y: top, width: right - left + 1, height: bottom - top + 1))
}

func contentRectFromCellBorders(
    source: RGBAImage,
    column: Int,
    row: Int,
    verticalGroups: [LineGroup],
    horizontalGroups: [LineGroup],
    padding: Int
) -> Rect {
    let leftBorder = verticalGroups[column * 2]
    let rightBorder = verticalGroups[column * 2 + 1]
    let topBorder = horizontalGroups[row * 2]
    let bottomBorder = horizontalGroups[row * 2 + 1]
    let left = min(source.width - 2, leftBorder.end + padding)
    let right = max(left + 1, rightBorder.start - padding)
    let top = min(source.height - 2, topBorder.end + padding)
    let bottom = max(top + 1, bottomBorder.start - padding)
    return Rect(x: left, y: top, width: right - left + 1, height: bottom - top + 1)
}

func cellOuterSize(
    column: Int,
    row: Int,
    verticalGroups: [LineGroup],
    horizontalGroups: [LineGroup]
) -> (width: Int, height: Int) {
    let leftBorder = verticalGroups[column * 2]
    let rightBorder = verticalGroups[column * 2 + 1]
    let topBorder = horizontalGroups[row * 2]
    let bottomBorder = horizontalGroups[row * 2 + 1]
    return (
        width: rightBorder.end - leftBorder.start + 1,
        height: bottomBorder.end - topBorder.start + 1
    )
}

func cellOuterRect(
    column: Int,
    row: Int,
    verticalGroups: [LineGroup],
    horizontalGroups: [LineGroup]
) -> Rect {
    let leftBorder = verticalGroups[column * 2]
    let rightBorder = verticalGroups[column * 2 + 1]
    let topBorder = horizontalGroups[row * 2]
    let bottomBorder = horizontalGroups[row * 2 + 1]
    return Rect(
        x: leftBorder.start,
        y: topBorder.start,
        width: rightBorder.end - leftBorder.start + 1,
        height: bottomBorder.end - topBorder.start + 1
    )
}

func isBackgroundLike(_ p: (r: UInt8, g: UInt8, b: UInt8, a: UInt8)) -> Bool {
    guard p.a > 0 else { return true }
    let minChannel = min(Int(p.r), Int(p.g), Int(p.b))
    let maxChannel = max(Int(p.r), Int(p.g), Int(p.b))
    let brightness = (Int(p.r) + Int(p.g) + Int(p.b)) / 3
    return brightness >= 232 && maxChannel - minChannel <= 32
}

func isBorderLike(_ p: (r: UInt8, g: UInt8, b: UInt8, a: UInt8)) -> Bool {
    guard p.a > 0 else { return false }
    let minChannel = min(Int(p.r), Int(p.g), Int(p.b))
    let maxChannel = max(Int(p.r), Int(p.g), Int(p.b))
    let brightness = (Int(p.r) + Int(p.g) + Int(p.b)) / 3
    return brightness <= 175 && maxChannel - minChannel <= 40
}

func removeEdgeConnectedPixels(from image: RGBAImage, where shouldRemove: ((r: UInt8, g: UInt8, b: UInt8, a: UInt8)) -> Bool) -> RGBAImage {
    var out = image
    let width = image.width
    let height = image.height
    var visited = [Bool](repeating: false, count: width * height)
    var queue: [(Int, Int)] = []

    func visit(_ x: Int, _ y: Int) {
        let idx = y * width + x
        if visited[idx] { return }
        visited[idx] = true
        let p = image.pixel(x: x, y: y)
        if shouldRemove(p) {
            queue.append((x, y))
        }
    }

    for x in 0..<width {
        visit(x, 0)
        visit(x, height - 1)
    }
    for y in 0..<height {
        visit(0, y)
        visit(width - 1, y)
    }

    var head = 0
    while head < queue.count {
        let (x, y) = queue[head]
        head += 1
        let p = out.pixel(x: x, y: y)
        out.setPixel(x: x, y: y, r: p.r, g: p.g, b: p.b, a: 0)
        if x > 0 { visit(x - 1, y) }
        if x + 1 < width { visit(x + 1, y) }
        if y > 0 { visit(x, y - 1) }
        if y + 1 < height { visit(x, y + 1) }
    }

    return out
}

func makeFrameTransparent(_ image: RGBAImage) -> RGBAImage {
    let withoutBorder = removeEdgeConnectedPixels(from: image, where: isBorderLike)
    return removeEdgeConnectedPixels(from: withoutBorder, where: isBackgroundLike)
}

func padToCanvas(_ image: RGBAImage, width: Int, height: Int) -> RGBAImage {
    var out = RGBAImage(width: width, height: height, data: [UInt8](repeating: 0, count: width * height * 4))
    let offsetX = (width - image.width) / 2
    let offsetY = (height - image.height) / 2
    for y in 0..<image.height {
        for x in 0..<image.width {
            let p = image.pixel(x: x, y: y)
            out.setPixel(x: offsetX + x, y: offsetY + y, r: p.r, g: p.g, b: p.b, a: p.a)
        }
    }
    return out
}

func contentBounds(_ image: RGBAImage, alphaThreshold: UInt8 = 16) -> Rect? {
    var minX = image.width
    var minY = image.height
    var maxX = -1
    var maxY = -1

    for y in 0..<image.height {
        for x in 0..<image.width {
            if image.pixel(x: x, y: y).a > alphaThreshold {
                minX = min(minX, x)
                minY = min(minY, y)
                maxX = max(maxX, x)
                maxY = max(maxY, y)
            }
        }
    }

    guard maxX >= 0 else {
        return nil
    }

    return Rect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1)
}

func padToCanvasCenteredOnContent(_ image: RGBAImage, width: Int, height: Int) -> (image: RGBAImage, bounds: Rect?, offsetX: Int, offsetY: Int) {
    let bounds = contentBounds(image)
    let offsetX: Int
    let offsetY: Int

    if let bounds {
        offsetX = Int(round((Double(width) - Double(bounds.width)) / 2.0)) - bounds.x
        offsetY = Int(round((Double(height) - Double(bounds.height)) / 2.0)) - bounds.y
    } else {
        offsetX = (width - image.width) / 2
        offsetY = (height - image.height) / 2
    }

    var out = RGBAImage(width: width, height: height, data: [UInt8](repeating: 0, count: width * height * 4))
    for y in 0..<image.height {
        for x in 0..<image.width {
            let targetX = offsetX + x
            let targetY = offsetY + y
            if targetX >= 0 && targetX < width && targetY >= 0 && targetY < height {
                let p = image.pixel(x: x, y: y)
                out.setPixel(x: targetX, y: targetY, r: p.r, g: p.g, b: p.b, a: p.a)
            }
        }
    }

    return (out, contentBounds(out), offsetX, offsetY)
}

func placeImageOnCanvas(_ image: RGBAImage, width: Int, height: Int, offsetX: Int, offsetY: Int) -> RGBAImage {
    var out = RGBAImage(width: width, height: height, data: [UInt8](repeating: 0, count: width * height * 4))
    for y in 0..<image.height {
        for x in 0..<image.width {
            let targetX = offsetX + x
            let targetY = offsetY + y
            if targetX >= 0 && targetX < width && targetY >= 0 && targetY < height {
                let p = image.pixel(x: x, y: y)
                out.setPixel(x: targetX, y: targetY, r: p.r, g: p.g, b: p.b, a: p.a)
            }
        }
    }
    return out
}

func padToCanvasPreservingCellPosition(
    _ image: RGBAImage,
    contentRect: Rect,
    cellRect: Rect,
    width: Int,
    height: Int
) -> (image: RGBAImage, bounds: Rect?, offsetX: Int, offsetY: Int) {
    let cellCenterX = Double(cellRect.x) + Double(cellRect.width) / 2.0
    let cellCenterY = Double(cellRect.y) + Double(cellRect.height) / 2.0
    let canvasCenterX = Double(width) / 2.0
    let canvasCenterY = Double(height) / 2.0
    let offsetX = Int(round(canvasCenterX + Double(contentRect.x) - cellCenterX))
    let offsetY = Int(round(canvasCenterY + Double(contentRect.y) - cellCenterY))
    let placed = placeImageOnCanvas(image, width: width, height: height, offsetX: offsetX, offsetY: offsetY)
    return (placed, contentBounds(placed), offsetX, offsetY)
}

func uniformCellRect(source: RGBAImage, column: Int, row: Int, columns: Int, rows: Int) -> Rect {
    let left = Int(round(Double(source.width * column) / Double(columns)))
    let right = Int(round(Double(source.width * (column + 1)) / Double(columns))) - 1
    let top = Int(round(Double(source.height * row) / Double(rows)))
    let bottom = Int(round(Double(source.height * (row + 1)) / Double(rows))) - 1
    return Rect(x: left, y: top, width: right - left + 1, height: bottom - top + 1)
}

func combineFrames(_ frames: [RGBAImage], columns: Int, rows: Int) -> RGBAImage {
    guard !frames.isEmpty else {
        return RGBAImage(width: 1, height: 1, data: [0, 0, 0, 0])
    }
    let frameWidth = frames[0].width
    let frameHeight = frames[0].height
    var sheet = RGBAImage(
        width: frameWidth * columns,
        height: frameHeight * rows,
        data: [UInt8](repeating: 0, count: frameWidth * columns * frameHeight * rows * 4)
    )
    for (index, frame) in frames.enumerated() {
        let col = index % columns
        let row = index / columns
        for y in 0..<frame.height {
            for x in 0..<frame.width {
                let p = frame.pixel(x: x, y: y)
                sheet.setPixel(x: col * frameWidth + x, y: row * frameHeight + y, r: p.r, g: p.g, b: p.b, a: p.a)
            }
        }
    }
    return sheet
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

do {
    let options = try parseOptions(CommandLine.arguments)
    let outputDir = options.outputDir
    let framesDir = outputDir + "/frames"
    try FileManager.default.createDirectory(atPath: framesDir, withIntermediateDirectories: true)

    let source = try loadImage(at: options.inputPath)
    let verticalGroups = lineGroups(in: 0..<source.width, threshold: options.borderThreshold) { darkPixelRatioForColumn(source, x: $0) }
    let horizontalGroups = lineGroups(in: 0..<source.height, threshold: options.borderThreshold) { darkPixelRatioForRow(source, y: $0) }
    let canUseBorderedGrid = verticalGroups.count >= 2
        && horizontalGroups.count >= 2
        && verticalGroups.count % 2 == 0
        && horizontalGroups.count % 2 == 0
    let useUniformGrid: Bool
    switch options.gridMode {
    case .uniform:
        useUniformGrid = true
    case .bordered:
        useUniformGrid = false
    case .auto:
        useUniformGrid = !canUseBorderedGrid
    }

    let columns: Int
    let rows: Int
    if useUniformGrid {
        if let optionColumns = options.columns, let optionRows = options.rows {
            columns = optionColumns
            rows = optionRows
        } else if let frameWidth = options.frameWidth, let frameHeight = options.frameHeight {
            guard source.width % frameWidth == 0, source.height % frameHeight == 0 else {
                throw NSError(
                    domain: "SpriteExtract",
                    code: 19,
                    userInfo: [NSLocalizedDescriptionKey: "Uniform grid needs --columns/--rows or a --frame-size that evenly divides the source image."]
                )
            }
            columns = source.width / frameWidth
            rows = source.height / frameHeight
        } else {
            throw NSError(
                domain: "SpriteExtract",
                code: 20,
                userInfo: [NSLocalizedDescriptionKey: "No bordered grid detected. For borderless sheets, pass --grid uniform with --columns/--rows or --frame-size."]
            )
        }
    } else {
        columns = options.columns ?? (verticalGroups.count / 2)
        rows = options.rows ?? (horizontalGroups.count / 2)
        guard verticalGroups.count == columns * 2, horizontalGroups.count == rows * 2 else {
            throw NSError(
                domain: "SpriteExtract",
                code: 4,
                userInfo: [NSLocalizedDescriptionKey: "Expected \(columns * 2) vertical and \(rows * 2) horizontal border groups, found V=\(verticalGroups.count) H=\(horizontalGroups.count). Try --grid uniform, --rows, --columns, or --border-threshold."]
            )
        }
    }

    let detectedFrameWidth = options.frameWidth ?? Int(ceil(Double(source.width) / Double(columns)))
    let detectedFrameHeight = options.frameHeight ?? Int(ceil(Double(source.height) / Double(rows)))
    let frameWidth = options.frameWidth ?? detectedFrameWidth
    let frameHeight = options.frameHeight ?? detectedFrameHeight

    var frames: [RGBAImage] = []
    var metadata: [FrameMetadata] = []
    let rowDigits = max(2, String(max(0, rows - 1)).count)
    let columnDigits = max(2, String(max(0, columns - 1)).count)

    for row in 0..<rows {
        for col in 0..<columns {
            let frameCell: FrameCell
            if useUniformGrid {
                let rect = uniformCellRect(source: source, column: col, row: row, columns: columns, rows: rows)
                frameCell = FrameCell(cellRect: rect, contentRect: rect)
            } else {
                let cellRect = cellOuterRect(
                    column: col,
                    row: row,
                    verticalGroups: verticalGroups,
                    horizontalGroups: horizontalGroups
                )
                let contentRect = contentRectFromCellBorders(
                    source: source,
                    column: col,
                    row: row,
                    verticalGroups: verticalGroups,
                    horizontalGroups: horizontalGroups,
                    padding: options.trimPadding
                )
                frameCell = FrameCell(cellRect: cellRect, contentRect: contentRect)
            }
            let contentRect = frameCell.contentRect
            let transparentFrame = makeFrameTransparent(crop(source, rect: contentRect))
            let transparentBounds = contentBounds(transparentFrame)
            if let transparentBounds {
                guard transparentBounds.width <= frameWidth, transparentBounds.height <= frameHeight else {
                    throw NSError(
                        domain: "SpriteExtract",
                        code: 16,
                        userInfo: [NSLocalizedDescriptionKey: "Frame \(row * columns + col) visible content is \(transparentBounds.width)x\(transparentBounds.height), larger than output frame \(frameWidth)x\(frameHeight). Increase --frame-size."]
                    )
                }
            }
            guard transparentFrame.width <= frameWidth || transparentBounds != nil,
                  transparentFrame.height <= frameHeight || transparentBounds != nil else {
                throw NSError(
                    domain: "SpriteExtract",
                    code: 16,
                    userInfo: [NSLocalizedDescriptionKey: "Frame \(row * columns + col) content is \(transparentFrame.width)x\(transparentFrame.height), larger than output frame \(frameWidth)x\(frameHeight). Increase --frame-size."]
                )
            }
            let placed: (image: RGBAImage, bounds: Rect?, offsetX: Int, offsetY: Int)
            switch options.alignment {
            case .preserveCell:
                placed = padToCanvasPreservingCellPosition(
                    transparentFrame,
                    contentRect: contentRect,
                    cellRect: frameCell.cellRect,
                    width: frameWidth,
                    height: frameHeight
                )
            case .centerContent:
                placed = padToCanvasCenteredOnContent(transparentFrame, width: frameWidth, height: frameHeight)
            }
            let filename = frameFilename(
                prefix: options.prefix,
                row: row,
                column: col,
                rowDigits: rowDigits,
                columnDigits: columnDigits
            )
            try savePNG(placed.image, to: framesDir + "/" + filename)
            frames.append(placed.image)
            let anchorX: Double
            let anchorY: Double
            if let bounds = placed.bounds {
                anchorX = (Double(bounds.x) + Double(bounds.width) / 2.0) / Double(frameWidth)
                anchorY = (Double(bounds.y) + Double(bounds.height) / 2.0) / Double(frameHeight)
            } else {
                anchorX = 0.5
                anchorY = 0.5
            }
            metadata.append(FrameMetadata(
                index: row * columns + col,
                row: row,
                column: col,
                filename: "frames/" + filename,
                sourceRect: contentRect,
                bounds: placed.bounds,
                offsetX: placed.offsetX,
                offsetY: placed.offsetY,
                anchorX: anchorX,
                anchorY: anchorY
            ))
        }
    }

    let sheet = combineFrames(frames, columns: columns, rows: rows)
    let sheetName = "\(options.spriteName).png"
    let sheetURL = URL(fileURLWithPath: outputDir + "/" + sheetName)
    try savePNG(sheet, to: sheetURL.path)

    let meta = SpriteSheetMetadata(
        image: sheetName,
        frameWidth: frameWidth,
        frameHeight: frameHeight,
        columns: columns,
        rows: rows,
        frameCount: frames.count,
        frames: metadata
    )
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    let json = try encoder.encode(meta)
    let jsonName = "\(options.spriteName).json"
    let jsonURL = URL(fileURLWithPath: outputDir + "/" + jsonName)
    try json.write(to: jsonURL)
    let installDir = try installSpriteAssets(imageURL: sheetURL, metadataURL: jsonURL)

    print("Created \(frames.count) frames in \(framesDir)")
    print("Grid: \(useUniformGrid ? "uniform" : "bordered") \(columns)x\(rows)")
    print("Frame size: \(frameWidth)x\(frameHeight)")
    print("Sprite sheet: \(sheetURL.path)")
    print("Metadata: \(jsonURL.path)")
    print("Installed: \(installDir.appendingPathComponent(sheetName).path)")
    print("Installed metadata: \(installDir.appendingPathComponent(jsonName).path)")
} catch {
    fputs("Error: \(error.localizedDescription)\n", stderr)
    exit(1)
}
