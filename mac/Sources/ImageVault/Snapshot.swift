import AppKit
import SwiftUI

/// Developer aid: `IMAGEVAULT_ROOT=/tmp/x ImageVault --snapshot <dir>` renders
/// each island state off-screen to PNGs (no screen-recording permission needed).
@MainActor
enum Snapshot {
    static func run(to dir: String) {
        let out = URL(fileURLWithPath: dir, isDirectory: true)
        try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

        // Seed a few sample images.
        if VaultStore.shared.load().items.isEmpty {
            for i in 0..<14 {
                let img = NSImage(size: NSSize(width: 480, height: 320), flipped: false) { r in
                    let g = NSGradient(starting: NSColor(white: CGFloat(i % 7) / 8 + 0.1, alpha: 1),
                                       ending: NSColor(white: 1 - CGFloat(i % 5) / 6, alpha: 1))
                    g?.draw(in: r, angle: CGFloat(i * 37))
                    NSString(string: "\(i + 1)").draw(at: NSPoint(x: 24, y: 200), withAttributes: [.font: NSFont.boldSystemFont(ofSize: 90), .foregroundColor: NSColor.black])
                    return true
                }
                let png = (img.tiffRepresentation.flatMap { NSBitmapImageRep(data: $0) })?
                    .representation(using: .png, properties: [:]) ?? Data()
                _ = try? VaultStore.shared.add(data: png, meta: .init(name: "sample-\(i + 1)", url: nil, pageTitle: nil, pageUrl: nil, domain: nil, source: "snapshot"))
            }
        }
        // Folder, SVG and GIF samples.
        if VaultStore.shared.load().groups?.isEmpty ?? true {
            let svg = #"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><circle cx="32" cy="32" r="28" fill="none" stroke="white" stroke-width="5"/><path d="M20 33l8 8 16-18" fill="none" stroke="white" stroke-width="5" stroke-linecap="round"/></svg>"#
            _ = try? VaultStore.shared.add(data: Data(svg.utf8), meta: .init(name: "logo", url: nil, pageTitle: nil, pageUrl: nil, domain: nil, source: "snapshot"))
            let ids = VaultStore.shared.load().items.suffix(5).map(\.id)
            VaultStore.shared.createGroup(name: "Client site", ids: ids)
        }
        VaultModel.shared.reload()

        let model = IslandModel()
        let tray = TrayController()
        model.position = .center
        model.notch = CGSize(width: 190, height: 32)
        model.expandedSize = CGSize(width: 660, height: model.band + 8 + 236)
        model.count = VaultModel.shared.items.count
        model.activityImage = VaultModel.shared.items.first.flatMap { NSImage(contentsOf: VaultStore.shared.fileURL(for: $0)) }
        model.activityTitle = "Saved"
        model.activityDetail = "\(model.count) in vault"
        tray.onSelection = { model.selection = $0 }
        tray.onVisibleCount = { model.visibleCount = $0 }
        NowPlaying.shared.previewSample()

        let host = IslandHostView(rootView: IslandView(model: model, tray: tray, actions: IslandActions(), nowPlaying: .shared))
        let o = model.outline(for: .expanded)
        let frame = NSRect(x: -6000, y: -6000, width: o.width + IslandController.pad * 2, height: o.height + IslandController.pad)
        let window = NSWindow(contentRect: frame, styleMask: .borderless, backing: .buffered, defer: false)
        window.backgroundColor = NSColor(white: 0.32, alpha: 1) // stand-in for a desktop so black reads
        window.contentView = host
        window.orderFrontRegardless()

        func pump(_ seconds: TimeInterval) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }
        func shoot(_ name: String) {
            pump(0.9)
            host.layoutSubtreeIfNeeded()
            let scale: CGFloat = 2
            let size = host.bounds.size
            guard let ctx = CGContext(data: nil, width: Int(size.width * scale), height: Int(size.height * scale), bitsPerComponent: 8,
                                      bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return }
            ctx.setFillColor(NSColor(white: 0.32, alpha: 1).cgColor)
            ctx.fill(CGRect(x: 0, y: 0, width: size.width * scale, height: size.height * scale))
            ctx.translateBy(x: 0, y: size.height * scale)
            ctx.scaleBy(x: scale, y: -scale)
            host.layer?.render(in: ctx)
            if let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) {
                host.cacheDisplay(in: host.bounds, to: rep)
                try? rep.representation(using: .png, properties: [:])?.write(to: out.appendingPathComponent("\(name)-cache.png"))
            }
            if let cg = ctx.makeImage(),
               let data = NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:]) {
                try? data.write(to: out.appendingPathComponent("\(name).png"))
            }
        }

        for (name, state) in [("1-collapsed", IslandState.collapsed), ("2-peek", .peek), ("3-activity", .activity),
                              ("4-draghint", .dragHint), ("5-dragover", .dragOver), ("6-expanded", .expanded)] {
            model.state = state
            shoot(name)
        }
        tray.snapshotHover(2)
        shoot("6a-hover")
        tray.snapshotHover(-1)
        if let gid = VaultModel.shared.groups.first?.id {
            tray.toggleFolder(gid)
            pump(0.16)
            shoot("6b-folder-opening")
            pump(1.2)
            shoot("6c-folder-open")
            tray.toggleFolder(gid)
            pump(1.6)
            shoot("6d-folder-closed-again")
        }
        tray.toggleSelectAll()
        model.message = "Copied — ⌘V anywhere"
        shoot("7-expanded-selected")
        tray.clearSelection()
        model.message = nil
        shoot("8-expanded-cleared")

        model.query = "sample"
        tray.setFilter(query: "sample", sort: .newest)
        shoot("9-expanded-search")

        model.query = "zzz"
        tray.setFilter(query: "zzz", sort: .newest)
        shoot("10-expanded-no-matches")

        model.query = ""
        tray.setFilter(query: "", sort: .newest)

        tray.previewLargest()
        shoot("12-preview")
        tray.closePreview()
        pump(0.4)

        ClipboardHistory.shared.loadSamples()
        model.tab = .clipboard
        shoot("11-clipboard")
        ClipboardGrid.snapshotExpand = ClipboardHistory.shared.items.first?.id
        model.tab = .vault
        pump(0.3)
        model.tab = .clipboard
        shoot("11b-clipboard-expanded")
        ClipboardGrid.snapshotExpand = nil
        model.tab = .vault
        print("snapshots in \(out.path)")
    }
}

/// `--capture-demo <image> <dir>`: exercises the screenshot renderer and windows.
@MainActor
enum CaptureDemo {
    static func run(image path: String, out: String) {
        guard let src = NSImage(contentsOfFile: path)?.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            print("can't read \(path)"); return
        }
        let dir = URL(fileURLWithPath: out)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let doc = CaptureDocument(image: src, scale: 2)
        let s = doc.size
        func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: s.width * x, y: s.height * y) }
        let red = EditorToolbar.colors[0].1, blue = EditorToolbar.colors[4].1, yellow = EditorToolbar.colors[2].1
        var text = Annotation(kind: .text, points: [p(0.06, 0.95)], color: .white, width: 4)
        text.text = "Text annotation"
        var pasted = Annotation(kind: .image, points: [p(0.72, 0.05), p(0.95, 0.25)], color: .white, width: 0)
        pasted.image = src
        doc.annotations = [
            Annotation(kind: .blur, points: [p(0.05, 0.05), p(0.3, 0.25)], color: .clear, width: 4),
            Annotation(kind: .spotlight, points: [p(0.35, 0.35), p(0.65, 0.65)], color: .clear, width: 4),
            Annotation(kind: .arrow, points: [p(0.1, 0.5), p(0.33, 0.6)], color: red, width: 4),
            Annotation(kind: .line, points: [p(0.1, 0.4), p(0.3, 0.4)], color: blue, width: 3),
            Annotation(kind: .measure, points: [p(0.4, 0.2), p(0.6, 0.2)], color: yellow, width: 4),
            Annotation(kind: .rectangle, points: [p(0.68, 0.6), p(0.9, 0.85)], color: red, width: 4),
            Annotation(kind: .filledRect, points: [p(0.7, 0.4), p(0.8, 0.48)], color: .black, width: 4),
            Annotation(kind: .oval, points: [p(0.4, 0.75), p(0.55, 0.9)], color: blue, width: 4),
            Annotation(kind: .pencil, points: (0..<30).map { i in p(0.05 + CGFloat(i) * 0.008, 0.75 + 0.05 * sin(CGFloat(i) / 3)) },
                       color: red, width: 4),
            Annotation(kind: .highlighter, points: [p(0.35, 0.3), p(0.6, 0.3)], color: yellow, width: 4),
            Annotation(kind: .counter, points: [p(0.62, 0.7)], color: red, width: 4),
            Annotation(kind: .counter, points: [p(0.66, 0.3)], color: blue, width: 4),
            text, pasted,
        ]
        write(doc.rendered, to: dir.appendingPathComponent("rendered.png"))
        // Resolution check: the export must be the capture's exact pixel size,
        // and pixels outside every mark must be untouched.
        let plain = CaptureDocument(image: src, scale: 1)
        plain.annotations = [Annotation(kind: .arrow, points: [CGPoint(x: 5, y: 5), CGPoint(x: 40, y: 40)], color: .red, width: 4)]
        let out = plain.rendered
        var same = 0, total = 0
        if let a = NSBitmapImageRep(cgImage: src).bitmapData, let b = NSBitmapImageRep(cgImage: out).bitmapData {
            let ra = NSBitmapImageRep(cgImage: src), rb = NSBitmapImageRep(cgImage: out)
            for y in stride(from: 100, to: src.height, by: 7) {
                for x in stride(from: 100, to: src.width, by: 7) {
                    total += 1
                    let ca = ra.colorAt(x: x, y: y)!, cb = rb.colorAt(x: x, y: y)!
                    if abs(ca.redComponent - cb.redComponent) < 0.004, abs(ca.greenComponent - cb.greenComponent) < 0.004,
                       abs(ca.blueComponent - cb.blueComponent) < 0.004 { same += 1 }
                }
            }
            _ = (a, b)
        }
        print("export \(out.width)×\(out.height) vs capture \(src.width)×\(src.height); untouched pixels identical: \(same)/\(total)")
        print("color at centre:", doc.color(at: p(0.5, 0.5))?.hexString ?? "nil")

        let preview = CapturePreview(document: doc, screen: NSScreen.screens[0])
        snapshot(preview, to: dir.appendingPathComponent("preview.png"))
        EditorWindowController.open(doc)
        if let editor = NSApp.windows.first(where: { $0.title == "Edit Screenshot" }) {
            snapshot(editor, to: dir.appendingPathComponent("editor.png"))
            print("editor window:", editor.frame.size)
            if let canvas = (editor.contentView?.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? EditorCanvas) {
                print("editor zoom: \(Int(canvas.zoomLevel * 100))%")
            }
        }
        print("written to \(dir.path)")
    }

    private static func snapshot(_ window: NSWindow, to url: URL) {
        guard let view = window.contentView else { return }
        window.orderFrontRegardless()
        view.layoutSubtreeIfNeeded()
        view.displayIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        let scale: CGFloat = 2
        let size = view.bounds.size
        guard let ctx = CGContext(data: nil, width: Int(size.width * scale), height: Int(size.height * scale), bitsPerComponent: 8,
                                  bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return }
        ctx.setFillColor(NSColor(white: 0.3, alpha: 1).cgColor)
        ctx.fill(CGRect(x: 0, y: 0, width: size.width * scale, height: size.height * scale))
        ctx.scaleBy(x: scale, y: scale)
        view.layer?.render(in: ctx)
        if let image = ctx.makeImage() { write(image, to: url) }
        window.orderOut(nil)
    }

    private static func write(_ image: CGImage, to url: URL) {
        try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: url)
    }
}
