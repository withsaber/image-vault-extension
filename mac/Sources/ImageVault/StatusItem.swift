import AppKit

/// Menu-bar companion for the island: a visible way back in, recent images,
/// capture toggles and Quit. Left-click opens/closes the island; right-click
/// shows the menu.
@MainActor
final class StatusItemController: NSObject, NSMenuDelegate {
    private let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let menu = NSMenu()
    private let island: IslandController
    private let clipboard: ClipboardWatcher
    private let screenshots: ScreenshotWatcher

    init(island: IslandController, clipboard: ClipboardWatcher, screenshots: ScreenshotWatcher) {
        self.island = island
        self.clipboard = clipboard
        self.screenshots = screenshots
        super.init()

        if let button = statusItem.button {
            let image = NSImage(systemSymbolName: "photo.on.rectangle", accessibilityDescription: "Image Vault")
            image?.isTemplate = true
            image?.size = NSSize(width: 16, height: 16)
            button.image = image
            button.toolTip = "Image Vault — click to open, right-click for settings"
            button.target = self
            button.action = #selector(clicked)
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
        menu.delegate = self
        menu.autoenablesItems = false
        menu.appearance = NSAppearance(named: .darkAqua)
    }

    @objc private func clicked() {
        guard let event = NSApp.currentEvent, let button = statusItem.button else { return }
        if event.type == .rightMouseUp || event.modifierFlags.contains(.control) {
            menuNeedsUpdate(menu)
            menu.popUp(positioning: nil,
                       at: NSPoint(x: 0, y: button.bounds.height + 4), in: button)
        } else if island.model.state == .expanded {
            island.collapse()
        } else {
            island.reassert()
            island.expand()
        }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        menu.appearance = NSAppearance(named: .darkAqua)

        menu.addItem(ClosureMenuItem(title: "Open Image Vault") { [weak self] in
            self?.island.reassert()
            self?.island.expand()
        })
        menu.addItem(ClosureMenuItem(title: "Copy Latest Image", key: "b",
                                     modifiers: [.control, .option]) { [weak self] in
            self?.copyLatest()
        })
        menu.addItem(ClosureMenuItem(title: "Take Screenshot  \(AppController.shotShortcut.label)") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { CaptureCenter.shared.takeScreenshot() }
        })

        let recent = NSMenu()
        recent.autoenablesItems = false
        recent.appearance = NSAppearance(named: .darkAqua)
        for item in VaultModel.shared.items.prefix(5) {
            let title = item.name.count > 44 ? String(item.name.prefix(43)) + "…" : item.name
            let entry = ClosureMenuItem(title: title) { [weak self] in self?.copy(item) }
            if let cg = Imaging.thumbnail(at: VaultStore.shared.fileURL(for: item), maxPixel: 40) {
                let thumb = NSImage(cgImage: cg, size: NSSize(width: 16, height: 16))
                entry.image = thumb
            }
            recent.addItem(entry)
        }
        if recent.items.isEmpty {
            let empty = NSMenuItem(title: "No images yet", action: nil, keyEquivalent: "")
            empty.isEnabled = false
            recent.addItem(empty)
        }
        let recentItem = NSMenuItem(title: "Recent", action: nil, keyEquivalent: "")
        recentItem.submenu = recent
        menu.addItem(recentItem)

        let music = NSMenu()
        music.autoenablesItems = false
        music.appearance = NSAppearance(named: .darkAqua)
        if let track = NowPlaying.shared.track {
            let now = NSMenuItem(title: "\(track.title) — \(track.subtitle)", action: nil, keyEquivalent: "")
            now.isEnabled = false
            music.addItem(now)
            music.addItem(.separator())
        }
        music.addItem(ClosureMenuItem(title: "Play / Pause") { NowPlaying.shared.playPause() })
        music.addItem(ClosureMenuItem(title: "Next") { NowPlaying.shared.next() })
        music.addItem(ClosureMenuItem(title: "Previous") { NowPlaying.shared.previous() })
        let musicMenu = NSMenuItem(title: "Music", action: nil, keyEquivalent: "")
        musicMenu.submenu = music
        menu.addItem(musicMenu)

        menu.addItem(.separator())
        let clipboardItem = ClosureMenuItem(title: "Save Copied Images", key: "", modifiers: []) { [weak self] in
            self?.clipboard.enabled.toggle()
        }
        clipboardItem.state = clipboard.enabled ? .on : .off
        menu.addItem(clipboardItem)

        let screenshotItem = ClosureMenuItem(title: "Save New Screenshots", key: "", modifiers: []) { [weak self] in
            self?.screenshots.enabled.toggle()
        }
        screenshotItem.state = screenshots.enabled ? .on : .off
        menu.addItem(screenshotItem)

        let musicItem = ClosureMenuItem(title: "Music Controls", key: "", modifiers: []) {
            NowPlaying.shared.enabled.toggle()
        }
        musicItem.state = NowPlaying.shared.enabled ? .on : .off
        menu.addItem(musicItem)

        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem(title: "Open Vault Folder") {
            NSWorkspace.shared.open(VaultStore.shared.imagesURL)
        })
        menu.addItem(.separator())

        let stats = NSMenuItem(title: statsTitle(), action: nil, keyEquivalent: "")
        stats.isEnabled = false
        menu.addItem(stats)
        menu.addItem(ClosureMenuItem(title: "Quit Image Vault") { NSApp.terminate(nil) })
    }

    func copyLatest() {
        guard let item = VaultModel.shared.items.first else {
            island.flash("Vault is empty")
            return
        }
        copy(item)
    }

    private func copy(_ item: VaultItem) {
        Clipboard.copy([item])
        island.flash("Copied — ⌘V anywhere")
    }

    private func statsTitle() -> String {
        let items = VaultModel.shared.items
        let bytes = items.reduce(Int64(0)) { $0 + Int64($1.bytes ?? 0) }
        let size = ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"
        return "Image Vault \(version)  ·  \(items.count) images  ·  \(size)"
    }
}
