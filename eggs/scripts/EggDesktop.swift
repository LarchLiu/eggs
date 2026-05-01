import Cocoa
import Darwin

let defaultSpriteSize: CGFloat = 251
let tickInterval: TimeInterval = 1.0 / 30.0
let animationInterval: TimeInterval = 0.18
let appDir = FileManager.default.homeDirectoryForCurrentUser
    .appendingPathComponent(".codex")
    .appendingPathComponent("eggs")
let userSpritePath = appDir.appendingPathComponent("spritesheet.png").path
let userMetadataPath = appDir.appendingPathComponent("spritesheet.json").path
let statePath = appDir.appendingPathComponent("state.txt").path
let bundledSpritePath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ""
let bundledMetadataPath = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : ""
let stateNames = [
    "unborn",
    "ready",
    "hatching",
    "hatched",
    "walk",
    "sleep",
    "eat",
    "drink",
    "play",
    "roar",
    "attack",
]
let defaultState = "unborn"

func normalizedState(_ value: String) -> String? {
    let state = value.trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()
        .replacingOccurrences(of: "_", with: "-")
    let aliases: [String: String] = [
        "0": "unborn",
        "egg": "unborn",
        "not-born": "unborn",
        "notborn": "unborn",
        "1": "ready",
        "waiting": "ready",
        "about-to-hatch": "ready",
        "2": "hatching",
        "birth": "hatching",
        "breaking": "hatching",
        "3": "hatched",
        "born": "hatched",
        "idle": "hatched",
        "4": "walk",
        "walking": "walk",
        "first-walk": "walk",
        "5": "sleep",
        "sleeping": "sleep",
        "睡觉": "sleep",
        "6": "eat",
        "eating": "eat",
        "chicken": "eat",
        "drumstick": "eat",
        "吃鸡腿": "eat",
        "吃": "eat",
        "7": "drink",
        "drinking": "drink",
        "water": "drink",
        "喝水": "drink",
        "喝": "drink",
        "8": "play",
        "playing": "play",
        "玩耍": "play",
        "玩": "play",
        "9": "roar",
        "roaring": "roar",
        "咆哮": "roar",
        "叫": "roar",
        "10": "attack",
        "attacking": "attack",
        "hit": "attack",
        "fight": "attack",
        "攻击": "attack",
        "打": "attack",
    ]
    let canonical = aliases[state] ?? state
    return stateNames.contains(canonical) ? canonical : nil
}

func readState() -> String {
    guard let value = try? String(contentsOfFile: statePath, encoding: .utf8) else {
        return defaultState
    }
    return normalizedState(value) ?? defaultState
}

func color(_ hex: UInt32) -> NSColor {
    let r = CGFloat((hex >> 16) & 0xff) / 255
    let g = CGFloat((hex >> 8) & 0xff) / 255
    let b = CGFloat(hex & 0xff) / 255
    return NSColor(calibratedRed: r, green: g, blue: b, alpha: 1)
}

func oval(_ rect: NSRect, fill: NSColor, stroke: NSColor? = nil, width: CGFloat = 1) {
    let path = NSBezierPath(ovalIn: rect)
    fill.setFill()
    path.fill()
    if let stroke {
        stroke.setStroke()
        path.lineWidth = width
        path.stroke()
    }
}

struct SpriteSheetInfo {
    let imagePath: String
    let metadataPath: String?
    let frameWidth: Int
    let frameHeight: Int
}

func activeSpritePaths() -> (image: String, metadata: String?) {
    if FileManager.default.fileExists(atPath: userSpritePath) {
        return (userSpritePath, FileManager.default.fileExists(atPath: userMetadataPath) ? userMetadataPath : nil)
    }
    return (bundledSpritePath, FileManager.default.fileExists(atPath: bundledMetadataPath) ? bundledMetadataPath : nil)
}

func metadataFrameSize(_ path: String?) -> (width: Int, height: Int)? {
    guard let path,
          let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let width = object["frameWidth"] as? NSNumber,
          let height = object["frameHeight"] as? NSNumber,
          width.intValue > 0,
          height.intValue > 0 else {
        return nil
    }
    return (width.intValue, height.intValue)
}

func loadSpriteSheetInfo() -> SpriteSheetInfo? {
    let paths = activeSpritePaths()
    guard !paths.image.isEmpty,
          let sheet = NSImage(contentsOfFile: paths.image),
          let cg = sheet.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
        return nil
    }
    let fallbackWidth = min(Int(defaultSpriteSize), cg.width)
    let fallbackHeight = min(Int(defaultSpriteSize), cg.height)
    let frameSize = metadataFrameSize(paths.metadata) ?? (fallbackWidth, fallbackHeight)
    return SpriteSheetInfo(
        imagePath: paths.image,
        metadataPath: paths.metadata,
        frameWidth: frameSize.width,
        frameHeight: frameSize.height
    )
}

final class EggView: NSView {
    var phase: CGFloat = 0
    var isDragging = false
    private var frameIndex = 0
    private var nextFrameAdvance = Date()
    private var frames: [NSImage] = []
    private let spriteInfo: SpriteSheetInfo?
    private var currentState = readState()
    private var nextStateCheck = Date()
    private var dragOffset = NSPoint.zero

    override var isOpaque: Bool { false }
    override var isFlipped: Bool { true }

    init(frame frameRect: NSRect, spriteInfo: SpriteSheetInfo?) {
        self.spriteInfo = spriteInfo
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
        frames = Self.loadFrames(spriteInfo: spriteInfo)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    static func loadFrames(spriteInfo: SpriteSheetInfo?) -> [NSImage] {
        guard let spriteInfo,
              let sheet = NSImage(contentsOfFile: spriteInfo.imagePath),
              let cg = sheet.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            return []
        }

        let frameWidth = spriteInfo.frameWidth
        let frameHeight = spriteInfo.frameHeight
        guard cg.width >= frameWidth, cg.height >= frameHeight else { return [] }
        let cols = cg.width / frameWidth
        let rows = cg.height / frameHeight
        var result: [NSImage] = []
        for row in 0..<rows {
            for col in 0..<cols {
                let rect = CGRect(x: col * frameWidth, y: row * frameHeight, width: frameWidth, height: frameHeight)
                guard let crop = cg.cropping(to: rect) else { continue }
                result.append(NSImage(cgImage: crop, size: NSSize(width: frameWidth, height: frameHeight)))
            }
        }
        return result
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.clear.setFill()
        bounds.fill()

        if !frames.isEmpty {
            checkState()
            let stateFrames = framesForCurrentState()
            stateFrames[frameIndex % stateFrames.count].draw(in: bounds)
            if Date() >= nextFrameAdvance {
                frameIndex += 1
                nextFrameAdvance = Date().addingTimeInterval(animationInterval)
            }
            return
        }

        let bob = sin(phase * 2) * 4
        oval(NSRect(x: 62, y: 194, width: 127, height: 16), fill: color(0x2f2a1e))
        oval(NSRect(x: 74, y: 34 + bob, width: 103, height: 165), fill: color(0xf3ecd2), stroke: color(0x222016), width: 3)
        for point in [NSPoint(x: 111, y: 76), NSPoint(x: 91, y: 122), NSPoint(x: 146, y: 119), NSPoint(x: 125, y: 161)] {
            oval(NSRect(x: point.x - 12, y: point.y - 10 + bob, width: 24, height: 20), fill: color(0x89a957))
        }
    }

    private func checkState() {
        guard Date() >= nextStateCheck else { return }
        let nextState = readState()
        if nextState != currentState {
            currentState = nextState
            frameIndex = 0
        }
        nextStateCheck = Date().addingTimeInterval(0.2)
    }

    private func framesForCurrentState() -> [NSImage] {
        guard !frames.isEmpty else { return [] }
        let stateIndex = stateNames.firstIndex(of: currentState) ?? 0
        let framesPerState = max(1, frames.count / stateNames.count)
        let start = stateIndex * framesPerState
        let end = min(start + framesPerState, frames.count)
        if start < end {
            return Array(frames[start..<end])
        }
        return frames
    }

    override func mouseDown(with event: NSEvent) {
        isDragging = true
        dragOffset = event.locationInWindow
    }

    override func mouseDragged(with event: NSEvent) {
        guard let window else { return }
        let mouse = NSEvent.mouseLocation
        window.setFrameOrigin(NSPoint(x: mouse.x - dragOffset.x, y: mouse.y - dragOffset.y))
    }

    override func mouseUp(with event: NSEvent) {
        isDragging = false
    }
}

final class EggController {
    private let window: NSWindow
    private let view: EggView
    private var x: CGFloat
    private var y: CGFloat
    private var vx: CGFloat
    private var vy: CGFloat
    private var phase: CGFloat = 0
    private var nextTurn = Date().addingTimeInterval(Double.random(in: 4...9))
    private let screenFrame: NSRect
    private let spriteWidth: CGFloat
    private let spriteHeight: CGFloat

    init() {
        screenFrame = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let spriteInfo = loadSpriteSheetInfo()
        spriteWidth = CGFloat(spriteInfo?.frameWidth ?? Int(defaultSpriteSize))
        spriteHeight = CGFloat(spriteInfo?.frameHeight ?? Int(defaultSpriteSize))
        let maxStartX = max(screenFrame.minX, screenFrame.maxX - spriteWidth)
        let minStartY = screenFrame.minY + 40
        let maxStartY = max(minStartY, screenFrame.maxY - spriteHeight)
        x = CGFloat.random(in: screenFrame.minX...maxStartX)
        y = CGFloat.random(in: minStartY...maxStartY)
        vx = Bool.random() ? CGFloat.random(in: 0.6...1.3) : -CGFloat.random(in: 0.6...1.3)
        vy = CGFloat.random(in: -0.25...0.25)

        view = EggView(frame: NSRect(x: 0, y: 0, width: spriteWidth, height: spriteHeight), spriteInfo: spriteInfo)
        window = NSWindow(
            contentRect: NSRect(x: x, y: y, width: spriteWidth, height: spriteHeight),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = false
        window.level = .floating
        window.ignoresMouseEvents = false
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        window.contentView = view
        window.orderFrontRegardless()
    }

    func start() {
        Timer.scheduledTimer(withTimeInterval: tickInterval, repeats: true) { [weak self] _ in
            self?.tick()
        }
    }

    private func tick() {
        x = window.frame.origin.x
        y = window.frame.origin.y

        if view.isDragging {
            phase += 0.16
            view.phase = phase
            view.needsDisplay = true
            return
        }

        if Date() > nextTurn {
            nextTurn = Date().addingTimeInterval(Double.random(in: 4...10))
            vx = Bool.random() ? CGFloat.random(in: 0.45...1.4) : -CGFloat.random(in: 0.45...1.4)
            vy = CGFloat.random(in: -0.35...0.35)
        }

        x += vx
        y += vy + sin(phase * 0.65) * 0.12

        if x <= screenFrame.minX || x >= screenFrame.maxX - spriteWidth {
            vx *= -1
            x = min(max(x, screenFrame.minX), screenFrame.maxX - spriteWidth)
        }
        if y <= screenFrame.minY + 24 || y >= screenFrame.maxY - spriteHeight {
            vy *= -1
            y = min(max(y, screenFrame.minY + 24), screenFrame.maxY - spriteHeight)
        }

        phase += 0.16
        view.phase = phase
        view.needsDisplay = true
        window.setFrameOrigin(NSPoint(x: x, y: y))
    }
}

signal(SIGTERM) { _ in
    NSApplication.shared.terminate(nil)
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let controller = EggController()
controller.start()
app.run()
