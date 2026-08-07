import AppKit

// A row of potted pixel plants floating above every other window: one per Claude
// Code session, blooming while Claude works and wilting when it needs you.

// MARK: - Layout

/// A working plant's level is its subagent count; a waiting or alerting one's is
/// its wilt stage. Planter caps both, so a pack only ever supplies three levels.
private let maxWorkingLevel = 2
private let maxWiltLevel = 2

struct Layout {
    var scale: CGFloat
    var showLabels: Bool

    var spriteW: CGFloat { CGFloat(Pack.builtin.width) * scale }
    var spriteH: CGFloat { CGFloat(Pack.builtin.height) * scale }
    /// A cell is only as wide as the art inside it, not the padded canvas.
    var plantW: CGFloat { CGFloat(Sprites.inkWidth) * scale }
    var inkInset: CGFloat { CGFloat(Sprites.inkMinX) * scale }
    var fontSize: CGFloat { max(9, round(scale * 3.2)) }
    var labelH: CGFloat { showLabels ? fontSize + 4 : 0 }
    var gap: CGFloat { scale * 2 }
    var height: CGFloat { spriteH + labelH }

    func font() -> NSFont {
        NSFont.monospacedSystemFont(ofSize: fontSize, weight: .semibold)
    }
}

// MARK: - View

final class PlanterView: NSView {
    var plants: [Plant] = []
    var frameIndex = 0
    var layout: Layout
    private var imageCache: [String: NSImage] = [:]

    init(layout: Layout) {
        self.layout = layout
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError() }

    private func image(for plant: Plant) -> NSImage {
        let level = plant.state == .working
            ? min(plant.agents, maxWorkingLevel)
            : min(max(plant.waitStage, 0), maxWiltLevel)
        let frames = Pack.builtin.frames(state: plant.state, level: level)
        let index = frameIndex % frames.count
        let key = "\(plant.state.rawValue)-\(index)-\(plant.hue)-\(level)"
        if let cached = imageCache[key] { return cached }
        let img = Pack.builtin.image(rows: frames[index], hue: plant.hue)
        imageCache[key] = img
        return img
    }

    /// Where each plant and its label sit, left to right. A long directory name
    /// widens its own cell rather than overlapping the plant beside it.
    ///
    /// Where the visible plant sits: used for hit testing and reordering.
    private(set) var plantRects: [NSRect] = []
    /// Where the full sprite canvas is drawn, shifted so its ink lands on the
    /// plant rect. Drawing wider than the cell is harmless: the overhang is
    /// transparent, and every plant's ink stays inside its own cell.
    private(set) var drawRects: [NSRect] = []
    private(set) var labelOrigins: [NSPoint] = []
    private(set) var contentSize: NSSize = .zero

    func layoutPlants() {
        plantRects = []
        drawRects = []
        labelOrigins = []
        var x: CGFloat = 0

        for plant in plants {
            let labelW = layout.showLabels ? attributedLabel(plant).size().width : 0
            let cellW = max(layout.plantW, labelW)
            let plantX = x + (cellW - layout.plantW) / 2
            plantRects.append(NSRect(
                x: plantX, y: layout.labelH,
                width: layout.plantW, height: layout.spriteH
            ))
            drawRects.append(NSRect(
                x: plantX - layout.inkInset, y: layout.labelH,
                width: layout.spriteW, height: layout.spriteH
            ))
            labelOrigins.append(NSPoint(x: x + (cellW - labelW) / 2, y: 1))
            x += cellW + layout.gap
        }

        contentSize = NSSize(
            width: max(x - layout.gap, layout.plantW),
            height: layout.height
        )
    }

    private func attributedLabel(_ plant: Plant) -> NSAttributedString {
        // A dark halo keeps the text legible over whatever window is underneath.
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.85)
        shadow.shadowBlurRadius = 2
        shadow.shadowOffset = .zero

        return NSAttributedString(string: plant.display, attributes: [
            .font: layout.font(),
            .foregroundColor: NSColor(hue: plant.hue, saturation: 0.35, brightness: 1.0, alpha: 1),
            .shadow: shadow,
        ])
    }

    override func draw(_ dirtyRect: NSRect) {
        NSGraphicsContext.current?.imageInterpolation = .none
        guard drawRects.count == plants.count else { return }

        for (i, plant) in plants.enumerated() {
            var rect = drawRects[i]
            // Lift the plant being reordered, so it is obvious which one you have.
            if isReordering, i == dragIndex { rect.origin.y += layout.scale * 2 }
            image(for: plant).draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1)
            guard layout.showLabels else { continue }
            attributedLabel(plant).draw(at: labelOrigins[i])
        }
    }

    // MARK: mouse

    /// Clicks pass through the empty space between plants, so the overlay never
    /// steals a click meant for the window underneath.
    override func hitTest(_ point: NSPoint) -> NSView? {
        // hitTest is handed a point in the superview's coordinates; converting
        // from a nil superview reads it as window coordinates, which for a
        // borderless window's content view is the same thing.
        let local = convert(point, from: superview)
        let padded = plantRects.map { $0.insetBy(dx: layout.scale, dy: layout.scale) }
        return padded.contains(where: { $0.contains(local) }) ? self : nil
    }

    // MARK: reordering

    /// True while a plant is being dragged along the row. The controller stops
    /// reloading from disk during that time, so the row cannot snap back mid-drag.
    private(set) var isReordering = false
    private var dragIndex: Int?

    private func index(at event: NSEvent) -> Int? {
        let local = convert(event.locationInWindow, from: nil)
        return plantRects.firstIndex { $0.insetBy(dx: layout.scale, dy: layout.scale).contains(local) }
    }

    private func nearestIndex(toX x: CGFloat) -> Int {
        guard !plantRects.isEmpty else { return 0 }
        var best = 0
        for i in plantRects.indices
        where abs(plantRects[i].midX - x) < abs(plantRects[best].midX - x) {
            best = i
        }
        return best
    }

    override func mouseDown(with event: NSEvent) {
        // Command turns a drag into a reorder; without it, you move the whole row.
        if event.modifierFlags.contains(.command), let i = index(at: event) {
            isReordering = true
            dragIndex = i
            needsDisplay = true
        } else {
            window?.performDrag(with: event)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard isReordering, let from = dragIndex else { return }
        let x = convert(event.locationInWindow, from: nil).x
        let to = nearestIndex(toX: x)
        guard to != from else { return }

        // Swapping as the cursor crosses each neighbour's centre adds up to
        // "move this plant to there", the same as dragging a browser tab.
        plants.swapAt(from, to)
        dragIndex = to
        layoutPlants()
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        guard isReordering else { return }
        isReordering = false
        dragIndex = nil
        Store.saveOrder(plants.map(\.sessionID))
        needsDisplay = true
    }

    override func rightMouseDown(with event: NSEvent) {
        let menu = NSMenu()
        menu.addItem(withTitle: "claude-planter", action: nil, keyEquivalent: "").isEnabled = false
        menu.addItem(withTitle: "drag to move · ⌘-drag to reorder", action: nil, keyEquivalent: "")
            .isEnabled = false
        menu.addItem(.separator())
        let labels = NSMenuItem(
            title: layout.showLabels ? "Hide labels" : "Show labels",
            action: #selector(toggleLabels), keyEquivalent: ""
        )
        labels.target = self
        menu.addItem(labels)
        let resetOrder = NSMenuItem(
            title: "Reset order", action: #selector(resetOrder), keyEquivalent: ""
        )
        resetOrder.target = self
        resetOrder.isEnabled = !Store.loadOrder().isEmpty
        menu.addItem(resetOrder)
        let quit = NSMenuItem(title: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        quit.target = NSApp
        menu.addItem(quit)
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }

    @objc private func toggleLabels() {
        layout.showLabels.toggle()
        Store.saveShowLabels(layout.showLabels)
        (window?.delegate as? OverlayController)?.refresh(force: true)
    }

    /// Back to creation order, oldest session first.
    @objc private func resetOrder() {
        Store.saveOrder([])
        (window?.delegate as? OverlayController)?.refresh(force: true)
    }
}

// MARK: - Window

final class OverlayWindow: NSWindow {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

final class OverlayController: NSObject, NSWindowDelegate {
    let window: OverlayWindow
    let view: PlanterView
    private var lastSignature = ""
    private var appliedOrigin: NSPoint?
    private var homeScreen: NSScreen?
    private var timer: Timer?
    private var tick = 0
    private let demoPlants: [Plant]?

    init(layout: Layout, demo: Bool) {
        view = PlanterView(layout: layout)
        window = OverlayWindow(
            // A placeholder frame; the first refresh sizes it to the real row.
            contentRect: NSRect(x: 0, y: 0, width: layout.plantW, height: layout.height),
            styleMask: .borderless, backing: .buffered, defer: false
        )
        demoPlants = demo ? OverlayController.makeDemoPlants() : nil
        super.init()

        window.contentView = view
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = false
        // Above ordinary windows, present on every Space, and skipped by cmd-tab
        // and Mission Control's window cycling.
        window.level = .floating
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        window.ignoresMouseEvents = false
        window.delegate = self
    }

    private static func makeDemoPlants() -> [Plant] {
        var plants = [
            Plant(sessionID: "demo-a", label: "monorepo", state: .working, createdAt: 1),
            Plant(sessionID: "demo-b", label: "helm", state: .waiting, createdAt: 2),
            Plant(sessionID: "demo-c", label: "recipes", state: .attention, createdAt: 3),
            Plant(sessionID: "demo-d", label: "planhub", agents: 2, state: .working, createdAt: 4),
        ]
        for (i, hue) in plantHues.prefix(plants.count).enumerated() { plants[i].hue = hue }
        return plants
    }

    func start() {
        refresh(force: true)
        timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.onTick()
        }
    }

    private func onTick() {
        tick += 1
        // One sprite frame per tick, so a bounce cycles twice a second. Reload
        // state half as often, which is still quick enough to feel immediate.
        view.frameIndex = tick
        let animated = view.plants.contains { $0.state == .working }
        refresh(force: animated, reload: tick % 2 == 0 && !view.isReordering)
    }

    func refresh(force: Bool = false, reload: Bool = true) {
        if reload { view.plants = demoPlants ?? Store.load() }

        // Only touch the window when something a viewer would notice changed.
        let signature = view.plants.map { "\($0.sessionID):\($0.state.rawValue):\($0.hue):\($0.display):\($0.agents):\($0.waitStage)" }
            .joined(separator: "|") + "|labels=\(view.layout.showLabels)"
        let changed = signature != lastSignature
        lastSignature = signature

        if changed { resize() }
        if changed || force { view.needsDisplay = true }
    }

    private func resize() {
        view.layoutPlants()
        guard !view.plants.isEmpty else {
            window.orderOut(nil)
            return
        }
        let size = view.contentSize
        let anchor = Store.loadAnchor(width: size.width) ?? defaultAnchor()
        // The row hangs from its right edge, so a new session widens it leftwards
        // and an overlay parked at the right-hand end of a screen stays put.
        let origin = clamp(
            NSPoint(x: anchor.right - size.width, y: anchor.bottom), size: size
        )
        // Resizing moves the window, which would otherwise be saved as if you had
        // dragged it there — and a row clamped to the screen edge as plants arrive
        // would then creep for good.
        appliedOrigin = origin
        window.setFrame(NSRect(origin: origin, size: size), display: true)
        if !window.isVisible { window.orderFrontRegardless() }
    }

    /// The bottom-right corner of the primary display. Deliberately not
    /// NSScreen.main: that follows the keyboard focus, so the row would land on a
    /// different display depending on where you happened to be looking when it
    /// started. Drag it anywhere; that is remembered and takes precedence.
    private func defaultAnchor() -> Store.Anchor {
        if homeScreen == nil { homeScreen = NSScreen.screens.first ?? NSScreen.main }
        guard let area = homeScreen?.visibleFrame else { return .init(right: 0, bottom: 0) }
        return Store.Anchor(right: area.maxX - 20, bottom: area.minY + 20)
    }

    /// Keeps the row on screen after a resolution or display change.
    private func clamp(_ origin: NSPoint, size: NSSize) -> NSPoint {
        let screen = NSScreen.screens.first { $0.frame.contains(origin) } ?? NSScreen.main
        guard let area = screen?.visibleFrame else { return origin }
        return NSPoint(
            x: min(max(origin.x, area.minX), max(area.minX, area.maxX - size.width)),
            y: min(max(origin.y, area.minY), max(area.minY, area.maxY - size.height))
        )
    }

    func windowDidMove(_ notification: Notification) {
        guard window.isVisible, window.frame.origin != appliedOrigin else { return }
        // Saved as a right edge, not a left one: the row must keep hanging from
        // where you dropped its right-hand end even as it gains plants.
        Store.saveAnchor(
            Store.Anchor(right: window.frame.maxX, bottom: window.frame.minY)
        )
    }
}

// MARK: - Preview sheet

/// Renders every frame in every colour to a PNG, over both a dark and a light
/// background. Used to eyeball the art without starting a session.
func writePreview(to path: String, scale: CGFloat = 6) {
    let pack = Pack.builtin
    let frames: [(String, [[String]])] = [
        ("working", pack.frames(state: .working, level: 0)),
        ("1 agent", [pack.frames(state: .working, level: 1)[0]]),
        ("2 agents", [pack.frames(state: .working, level: 2)[0]]),
        ("waiting", pack.frames(state: .waiting, level: 0)),
        ("wilt 2min", pack.frames(state: .waiting, level: 1)),
        ("wilt 10min", pack.frames(state: .waiting, level: 2)),
        ("attention", pack.frames(state: .attention, level: 0)),
    ]
    let flat = frames.flatMap { name, list in list.enumerated().map { ("\(name)\($0.offset + 1)", $0.element) } }

    let cellW = CGFloat(pack.width) * scale
    let cellH = CGFloat(pack.height) * scale + 16
    let cols = flat.count
    let rows = plantHues.count
    let panelW = CGFloat(cols) * cellW
    let size = NSSize(width: panelW * 2, height: CGFloat(rows) * cellH)

    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(size.width), pixelsHigh: Int(size.height),
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
    )!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSGraphicsContext.current?.imageInterpolation = .none

    NSColor(white: 0.12, alpha: 1).setFill()
    NSRect(x: 0, y: 0, width: panelW, height: size.height).fill()
    NSColor(white: 0.92, alpha: 1).setFill()
    NSRect(x: panelW, y: 0, width: panelW, height: size.height).fill()

    for (r, hue) in plantHues.enumerated() {
        for (c, entry) in flat.enumerated() {
            let img = pack.image(rows: entry.1, hue: hue)
            for panel in 0..<2 {
                let rect = NSRect(
                    x: CGFloat(panel) * panelW + CGFloat(c) * cellW,
                    y: size.height - CGFloat(r + 1) * cellH + 16,
                    width: cellW, height: CGFloat(pack.height) * scale
                )
                img.draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1)
                let text = NSAttributedString(string: entry.0, attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 10, weight: .medium),
                    .foregroundColor: panel == 0 ? NSColor.white : NSColor.black,
                ])
                text.draw(at: NSPoint(x: rect.minX + 4, y: rect.minY - 13))
            }
        }
    }

    NSGraphicsContext.restoreGraphicsState()
    guard let png = rep.representation(using: .png, properties: [:]) else { return }
    try? png.write(to: URL(fileURLWithPath: path))
    print("wrote \(path)")
}

// MARK: - Entry

func printPlants() {
    let plants = Store.load()
    if plants.isEmpty {
        print("no live sessions (state dir: \(Store.dir.path))")
        return
    }
    func pad(_ s: String, _ width: Int) -> String {
        s.count >= width ? s + " " : s + String(repeating: " ", count: width - s.count)
    }
    let labelWidth = (plants.map(\.label.count).max() ?? 8) + 2
    for plant in plants {
        let state = plant.state == .working
            ? plant.state.rawValue + (plant.agents > 0 ? " +\(plant.agents)" : "")
            : plant.state.rawValue + " \(plant.waitStage)"
        print(pad(plant.label, labelWidth) + pad(state, 14)
            + String(format: "hue=%.2f  ", Double(plant.hue)) + plant.sessionID)
    }
}

let args = Array(CommandLine.arguments.dropFirst())

func flagValue(_ name: String) -> String? {
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { return nil }
    return args[i + 1]
}

if args.contains("--help") || args.contains("-h") {
    print("""
    planter — a row of pixel plants, one per Claude Code session

      planter                 run the overlay
      planter --list          print the live sessions as text
      planter --demo          run the overlay with four fake plants
      planter --preview FILE  render all frames and colours to a PNG
      planter --scale N       pixel size, default 3
      planter --no-labels     hide the directory labels

    A plant blooms while its session works and wilts when it needs you.
    State lives in ~/.claude/planter (override with CLAUDE_PLANTER_DIR).

    Drag a plant to move the row; ⌘-drag to reorder. Right-click for labels,
    order reset, and quit.
    """)
    exit(0)
}

if args.contains("--list") {
    printPlants()
    exit(0)
}

if let path = flagValue("--preview") {
    writePreview(to: path)
    exit(0)
}

// --no-labels wins for this run; otherwise the last right-click choice stands.
let layout = Layout(
    scale: CGFloat(Double(flagValue("--scale") ?? "") ?? 3),
    showLabels: args.contains("--no-labels") ? false : (Store.loadShowLabels() ?? true)
)

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let demo = args.contains("--demo")
let controller = OverlayController(layout: layout, demo: demo)
controller.start()
if demo {
    // Enough to find and capture the window when checking how it renders.
    let f = controller.window.frame
    FileHandle.standardError.write(
        "demo: window \(controller.window.windowNumber) at \(Int(f.minX)),\(Int(f.minY)) \(Int(f.width))x\(Int(f.height))\n"
            .data(using: .utf8)!
    )
}
if controller.view.plants.isEmpty {
    FileHandle.standardError.write("watching \(Store.dir.path) — no sessions yet\n".data(using: .utf8)!)
}
app.run()
