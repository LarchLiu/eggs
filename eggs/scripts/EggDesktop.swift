import Cocoa
import Darwin

let defaultSpriteSize: CGFloat = 251
let tickInterval: TimeInterval = 1.0 / 30.0
let animationInterval: TimeInterval = 0.18
let appDir = FileManager.default.homeDirectoryForCurrentUser
    .appendingPathComponent(".codex")
    .appendingPathComponent("eggs")
let statePath = appDir.appendingPathComponent("state.json").path
let configPath = appDir.appendingPathComponent("config.json").path
let remotePeersPath = appDir.appendingPathComponent("remote-peers.json").path
let bundledAssetsPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ""
let defaultAnimations = [
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
let defaultSprite = "dino"

struct RuntimeState {
    let sprite: String
    let state: String
}

struct AnimationSpec {
    let name: String
    let row: Int
    let loop: Any
}

struct RemotePeerSnapshot {
    let peerID: String
    let state: String
    let sprite: String
    let imagePath: String
    let metadataPath: String?
    let configPath: String?
}

func normalizedKey(_ value: String) -> String {
    value.trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()
        .replacingOccurrences(of: "_", with: "-")
}

func configuredAnimationSpecs(spriteName: String?) -> [AnimationSpec] {
    let sprite = normalizedSprite(spriteName)
    guard let data = try? Data(contentsOf: URL(fileURLWithPath: configPath)),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let animations = object["animations"] as? [String: Any],
          let spriteAnimations = animations[sprite] as? [String: Any] else {
        return []
    }
    return spriteAnimations.compactMap { name, value in
        guard let spec = value as? [String: Any] else { return nil }
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty else { return nil }
        let row = (spec["row"] as? NSNumber)?.intValue ?? 0
        let loop = spec["loop"] ?? true
        return AnimationSpec(name: cleanName, row: max(0, row), loop: loop)
    }
}

func animationNames(spriteName: String?) -> [String] {
    let names = configuredAnimationSpecs(spriteName: spriteName).map(\.name)
    return names.isEmpty ? defaultAnimations : names
}

func defaultStateForSprite(_ spriteName: String?) -> String {
    animationNames(spriteName: spriteName).first ?? defaultState
}

func normalizedState(_ value: String, spriteName: String? = nil) -> String? {
    let state = normalizedKey(value)
    let names = animationNames(spriteName: spriteName)
    if let direct = names.first(where: { normalizedKey($0) == state }) {
        return direct
    }
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
    if let match = names.first(where: { normalizedKey($0) == canonical }) {
        return match
    }
    if names != defaultAnimations, let fallback = defaultAnimations.first(where: { normalizedKey($0) == canonical }) {
        return fallback
    }
    return nil
}

func readState() -> String {
    return readRuntimeState().state
}

func normalizedSprite(_ value: String?) -> String {
    guard let value else { return defaultSprite }
    let stem = URL(fileURLWithPath: value.trimmingCharacters(in: .whitespacesAndNewlines)).deletingPathExtension().lastPathComponent
    let allowed = stem.filter { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }
    return allowed.isEmpty ? defaultSprite : String(allowed)
}

func readRuntimeState() -> RuntimeState {
    guard let data = try? Data(contentsOf: URL(fileURLWithPath: statePath)) else {
        return RuntimeState(sprite: defaultSprite, state: defaultStateForSprite(defaultSprite))
    }
    guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        return RuntimeState(sprite: defaultSprite, state: defaultStateForSprite(defaultSprite))
    }
    var sprite = defaultSprite
    var state = defaultStateForSprite(sprite)
    if let rawSprite = object["sprite"] as? String {
        sprite = normalizedSprite(rawSprite)
    }
    if let rawState = object["state"] as? String {
        state = normalizedState(rawState, spriteName: sprite) ?? defaultStateForSprite(sprite)
    }
    return RuntimeState(sprite: sprite, state: state)
}

func readRemotePeers() -> [RemotePeerSnapshot] {
    guard let data = try? Data(contentsOf: URL(fileURLWithPath: remotePeersPath)),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let enabled = object["enabled"] as? Bool,
          enabled,
          let connected = object["connected"] as? Bool,
          connected,
          let peers = object["peers"] as? [[String: Any]] else {
        return []
    }
    return peers.compactMap { item in
        let peerID = String(describing: item["peer_id"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let sprite = normalizedSprite(item["sprite"] as? String)
        let state = normalizedState(item["state"] as? String ?? "", spriteName: sprite) ?? defaultStateForSprite(sprite)
        let imagePath = String(describing: item["image_path"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let metadataPath = (item["metadata_path"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let configPath = (item["config_path"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !peerID.isEmpty, !imagePath.isEmpty else { return nil }
        return RemotePeerSnapshot(
            peerID: peerID,
            state: state,
            sprite: sprite,
            imagePath: imagePath,
            metadataPath: (metadataPath?.isEmpty == false) ? metadataPath : nil,
            configPath: (configPath?.isEmpty == false) ? configPath : nil
        )
    }
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
    let name: String
    let imagePath: String
    let metadataPath: String?
    let frameWidth: Int
    let frameHeight: Int
    let columns: Int
    let rows: Int
}

func activeSpritePaths(spriteName: String) -> (image: String, metadata: String?) {
    let sprite = normalizedSprite(spriteName)
    let userImage = appDir.appendingPathComponent("\(sprite).png").path
    let userMetadata = appDir.appendingPathComponent("\(sprite).json").path
    if FileManager.default.fileExists(atPath: userImage) {
        return (userImage, FileManager.default.fileExists(atPath: userMetadata) ? userMetadata : nil)
    }
    let bundledAssets = URL(fileURLWithPath: bundledAssetsPath)
    let bundledImage = bundledAssets.appendingPathComponent("\(sprite).png").path
    let bundledMetadata = bundledAssets.appendingPathComponent("\(sprite).json").path
    return (bundledImage, FileManager.default.fileExists(atPath: bundledMetadata) ? bundledMetadata : nil)
}

func metadataInfo(_ path: String?) -> (width: Int, height: Int, columns: Int?, rows: Int?)? {
    guard let path,
          let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let width = object["frameWidth"] as? NSNumber,
          let height = object["frameHeight"] as? NSNumber,
          width.intValue > 0,
          height.intValue > 0 else {
        return nil
    }
    return (
        width.intValue,
        height.intValue,
        (object["columns"] as? NSNumber)?.intValue,
        (object["rows"] as? NSNumber)?.intValue
    )
}

func loadSpriteSheetInfo(spriteName: String) -> SpriteSheetInfo? {
    let sprite = normalizedSprite(spriteName)
    let paths = activeSpritePaths(spriteName: sprite)
    guard !paths.image.isEmpty,
          let sheet = NSImage(contentsOfFile: paths.image),
          let cg = sheet.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
        return nil
    }
    let fallbackWidth = min(Int(defaultSpriteSize), cg.width)
    let fallbackHeight = min(Int(defaultSpriteSize), cg.height)
    let metadata = metadataInfo(paths.metadata)
    let frameWidth = metadata?.width ?? fallbackWidth
    let frameHeight = metadata?.height ?? fallbackHeight
    return SpriteSheetInfo(
        name: sprite,
        imagePath: paths.image,
        metadataPath: paths.metadata,
        frameWidth: frameWidth,
        frameHeight: frameHeight,
        columns: metadata?.columns ?? max(1, cg.width / frameWidth),
        rows: metadata?.rows ?? max(1, cg.height / frameHeight)
    )
}

func loadSpriteSheetInfo(imagePath: String, metadataPath: String?, spriteName: String) -> SpriteSheetInfo? {
    guard let sheet = NSImage(contentsOfFile: imagePath),
          let cg = sheet.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
        return nil
    }
    let fallbackWidth = min(Int(defaultSpriteSize), cg.width)
    let fallbackHeight = min(Int(defaultSpriteSize), cg.height)
    let metadata = metadataInfo(metadataPath)
    let frameWidth = metadata?.width ?? fallbackWidth
    let frameHeight = metadata?.height ?? fallbackHeight
    return SpriteSheetInfo(
        name: normalizedSprite(spriteName),
        imagePath: imagePath,
        metadataPath: metadataPath,
        frameWidth: frameWidth,
        frameHeight: frameHeight,
        columns: metadata?.columns ?? max(1, cg.width / frameWidth),
        rows: metadata?.rows ?? max(1, cg.height / frameHeight)
    )
}

func animationSpecFromConfig(state: String, spriteName: String, configPath: String?) -> (row: Int, loop: Any) {
    guard let configPath,
          let data = try? Data(contentsOf: URL(fileURLWithPath: configPath)),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let animations = object["animations"] as? [String: Any],
          let spriteAnimations = animations[normalizedSprite(spriteName)] as? [String: Any] else {
        let row = defaultAnimations.map(normalizedKey).firstIndex(of: normalizedKey(state)) ?? 0
        return (row, true)
    }
    let stateKey = normalizedKey(state)
    for (name, value) in spriteAnimations {
        guard normalizedKey(name) == stateKey, let spec = value as? [String: Any] else { continue }
        let row = max(0, (spec["row"] as? NSNumber)?.intValue ?? 0)
        let loop = spec["loop"] ?? true
        return (row, loop)
    }
    let row = defaultAnimations.map(normalizedKey).firstIndex(of: normalizedKey(state)) ?? 0
    return (row, true)
}

final class EggView: NSView {
    var phase: CGFloat = 0
    var isDragging = false
    private var frameIndex = 0
    private var nextFrameAdvance = Date()
    private var frames: [NSImage] = []
    private var spriteInfo: SpriteSheetInfo?
    private var currentRuntimeState = readRuntimeState()
    private var nextStateCheck = Date()
    private var dragOffset = NSPoint.zero
    var onSpriteSizeChanged: ((CGFloat, CGFloat) -> Void)?

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
        checkState()

        if !frames.isEmpty {
            let (stateFrames, loop) = framesForCurrentState()
            let framePosition: Int
            let shouldAdvance: Bool
            if let loopBool = loop as? Bool {
                framePosition = loopBool ? frameIndex % stateFrames.count : min(frameIndex, stateFrames.count - 1)
                shouldAdvance = loopBool || frameIndex < stateFrames.count - 1
            } else if let loopNumber = loop as? NSNumber {
                let loopCount = max(1, loopNumber.intValue)
                let maxFrames = stateFrames.count * loopCount
                framePosition = frameIndex < maxFrames ? frameIndex % stateFrames.count : stateFrames.count - 1
                shouldAdvance = frameIndex < maxFrames - 1
            } else {
                framePosition = frameIndex % stateFrames.count
                shouldAdvance = true
            }
            stateFrames[framePosition].draw(in: bounds)
            if shouldAdvance && Date() >= nextFrameAdvance {
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
        let nextRuntimeState = readRuntimeState()
        if nextRuntimeState.sprite != currentRuntimeState.sprite {
            spriteInfo = loadSpriteSheetInfo(spriteName: nextRuntimeState.sprite)
            frames = Self.loadFrames(spriteInfo: spriteInfo)
            if let spriteInfo {
                onSpriteSizeChanged?(CGFloat(spriteInfo.frameWidth), CGFloat(spriteInfo.frameHeight))
            }
            currentRuntimeState = nextRuntimeState
            frameIndex = 0
        } else if nextRuntimeState.state != currentRuntimeState.state {
            currentRuntimeState = nextRuntimeState
            frameIndex = 0
        }
        nextStateCheck = Date().addingTimeInterval(0.2)
    }

    private func animationSpecForCurrentState() -> (row: Int, loop: Any) {
        let configured = configuredAnimationSpecs(spriteName: currentRuntimeState.sprite)
        let stateKey = normalizedKey(currentRuntimeState.state)
        if let spec = configured.first(where: { normalizedKey($0.name) == stateKey }) {
            return (spec.row, spec.loop)
        }
        let row = defaultAnimations.map(normalizedKey).firstIndex(of: stateKey) ?? 0
        return (row, true)
    }

    private func framesForCurrentState() -> ([NSImage], Any) {
        guard !frames.isEmpty else { return ([], true) }
        let spec = animationSpecForCurrentState()
        let columns = max(1, spriteInfo?.columns ?? frames.count)
        let rows = max(1, spriteInfo?.rows ?? 1)
        let row = min(max(0, spec.row), rows - 1)
        let start = row * columns
        let end = min(start + columns, frames.count)
        if start < end {
            return (Array(frames[start..<end]), spec.loop)
        }
        return (frames, spec.loop)
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

final class RemoteEggView: NSView {
    private var snapshot: RemotePeerSnapshot
    private var spriteInfo: SpriteSheetInfo?
    private var frames: [NSImage] = []
    private var frameIndex = 0
    private var nextFrameAdvance = Date()
    var mirrored = false

    override var isOpaque: Bool { false }
    override var isFlipped: Bool { true }

    init(frame frameRect: NSRect, snapshot: RemotePeerSnapshot) {
        self.snapshot = snapshot
        self.spriteInfo = loadSpriteSheetInfo(imagePath: snapshot.imagePath, metadataPath: snapshot.metadataPath, spriteName: snapshot.sprite)
        self.frames = EggView.loadFrames(spriteInfo: self.spriteInfo)
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func update(snapshot: RemotePeerSnapshot) {
        let spriteChanged = snapshot.imagePath != self.snapshot.imagePath || snapshot.metadataPath != self.snapshot.metadataPath
        self.snapshot = snapshot
        if spriteChanged {
            spriteInfo = loadSpriteSheetInfo(imagePath: snapshot.imagePath, metadataPath: snapshot.metadataPath, spriteName: snapshot.sprite)
            frames = EggView.loadFrames(spriteInfo: spriteInfo)
            frameIndex = 0
        }
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.clear.setFill()
        bounds.fill()
        guard !frames.isEmpty else { return }
        let spec = animationSpecFromConfig(state: snapshot.state, spriteName: snapshot.sprite, configPath: snapshot.configPath)
        let columns = max(1, spriteInfo?.columns ?? frames.count)
        let rows = max(1, spriteInfo?.rows ?? 1)
        let row = min(max(0, spec.row), rows - 1)
        let start = row * columns
        let end = min(start + columns, frames.count)
        let stateFrames = start < end ? Array(frames[start..<end]) : frames
        guard !stateFrames.isEmpty else { return }
        let framePosition: Int
        let shouldAdvance: Bool
        if let loopBool = spec.loop as? Bool {
            framePosition = loopBool ? frameIndex % stateFrames.count : min(frameIndex, stateFrames.count - 1)
            shouldAdvance = loopBool || frameIndex < stateFrames.count - 1
        } else if let loopNumber = spec.loop as? NSNumber {
            let loopCount = max(1, loopNumber.intValue)
            let maxFrames = stateFrames.count * loopCount
            framePosition = frameIndex < maxFrames ? frameIndex % stateFrames.count : stateFrames.count - 1
            shouldAdvance = frameIndex < maxFrames - 1
        } else {
            framePosition = frameIndex % stateFrames.count
            shouldAdvance = true
        }
        let frameImage = stateFrames[framePosition]
        if mirrored {
            NSGraphicsContext.saveGraphicsState()
            let transform = NSAffineTransform()
            transform.translateX(by: bounds.width, yBy: 0)
            transform.scaleX(by: -1, yBy: 1)
            transform.concat()
            frameImage.draw(in: bounds)
            NSGraphicsContext.restoreGraphicsState()
        } else {
            frameImage.draw(in: bounds)
        }
        if shouldAdvance && Date() >= nextFrameAdvance {
            frameIndex += 1
            nextFrameAdvance = Date().addingTimeInterval(animationInterval)
        }
    }
}

final class RemoteEggActor {
    let peerID: String
    let window: NSWindow
    let view: RemoteEggView
    var spriteWidth: CGFloat
    var spriteHeight: CGFloat
    var offsetDistance: CGFloat
    private(set) var isVisible = false

    init(snapshot: RemotePeerSnapshot) {
        peerID = snapshot.peerID
        let spriteInfo = loadSpriteSheetInfo(imagePath: snapshot.imagePath, metadataPath: snapshot.metadataPath, spriteName: snapshot.sprite)
        spriteWidth = CGFloat(spriteInfo?.frameWidth ?? Int(defaultSpriteSize))
        spriteHeight = CGFloat(spriteInfo?.frameHeight ?? Int(defaultSpriteSize))
        offsetDistance = CGFloat.random(in: 120...220)
        view = RemoteEggView(frame: NSRect(x: 0, y: 0, width: spriteWidth, height: spriteHeight), snapshot: snapshot)
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: spriteWidth, height: spriteHeight),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = false
        window.level = .floating
        window.ignoresMouseEvents = true
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        window.contentView = view
        window.orderFrontRegardless()
        isVisible = true
    }

    func update(snapshot: RemotePeerSnapshot, anchorX: CGFloat, anchorY: CGFloat, anchorHeight: CGFloat, screenFrame: NSRect) {
        view.update(snapshot: snapshot)
        spriteWidth = view.frame.width
        spriteHeight = view.frame.height
        view.setFrameSize(NSSize(width: spriteWidth, height: spriteHeight))
        window.setContentSize(NSSize(width: spriteWidth, height: spriteHeight))
        let x = min(max(anchorX + offsetDistance, screenFrame.minX), max(screenFrame.minX, screenFrame.maxX - spriteWidth))
        let localBottom = anchorY + anchorHeight
        let y = min(max(localBottom - spriteHeight, screenFrame.minY + 24), max(screenFrame.minY + 24, screenFrame.maxY - spriteHeight))
        view.mirrored = x > anchorX
        window.setFrameOrigin(NSPoint(x: x, y: y))
        if !isVisible {
            window.orderFrontRegardless()
            isVisible = true
        }
    }

    func hide() {
        guard isVisible else { return }
        window.orderOut(nil)
        isVisible = false
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
    private var spriteWidth: CGFloat
    private var spriteHeight: CGFloat
    private var remoteActors: [String: RemoteEggActor] = [:]
    private var hiddenRemoteActors: [String: RemoteEggActor] = [:]
    private var nextRemoteCheck = Date()

    init() {
        screenFrame = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let runtimeState = readRuntimeState()
        let spriteInfo = loadSpriteSheetInfo(spriteName: runtimeState.sprite)
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
        view.onSpriteSizeChanged = { [weak self] width, height in
            self?.resizeSprite(width: width, height: height)
        }
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
            updateRemoteActors()
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
        updateRemoteActors()
    }

    private func resizeSprite(width: CGFloat, height: CGFloat) {
        spriteWidth = width
        spriteHeight = height
        view.setFrameSize(NSSize(width: width, height: height))
        window.setContentSize(NSSize(width: width, height: height))
        x = min(max(window.frame.origin.x, screenFrame.minX), max(screenFrame.minX, screenFrame.maxX - spriteWidth))
        y = min(max(window.frame.origin.y, screenFrame.minY + 24), max(screenFrame.minY + 24, screenFrame.maxY - spriteHeight))
        window.setFrameOrigin(NSPoint(x: x, y: y))
    }

    private func updateRemoteActors() {
        guard Date() >= nextRemoteCheck else { return }
        nextRemoteCheck = Date().addingTimeInterval(0.2)
        let peers = readRemotePeers()
        let ids = Set(peers.map(\.peerID))
        let stalePeerIDs = remoteActors.keys.filter { !ids.contains($0) }
        for peerID in stalePeerIDs {
            guard let actor = remoteActors[peerID] else { continue }
            actor.hide()
            hiddenRemoteActors[peerID] = actor
            remoteActors.removeValue(forKey: peerID)
        }
        for peer in peers {
            let actor = remoteActors[peer.peerID] ?? {
                if let existing = hiddenRemoteActors.removeValue(forKey: peer.peerID) {
                    remoteActors[peer.peerID] = existing
                    return existing
                }
                let created = RemoteEggActor(snapshot: peer)
                remoteActors[peer.peerID] = created
                return created
            }()
            actor.update(snapshot: peer, anchorX: x, anchorY: y, anchorHeight: spriteHeight, screenFrame: screenFrame)
        }
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
