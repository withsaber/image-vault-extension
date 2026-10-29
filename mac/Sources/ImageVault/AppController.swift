import AppKit
import Carbon.HIToolbox
import ServiceManagement

/// Global shortcut via Carbon (no Accessibility permission needed).
final class HotKey {
    private static var handlers: [UInt32: () -> Void] = [:]
    private static var installed = false
    private var ref: EventHotKeyRef?
    private let id: UInt32
    /// False when another app already owns this shortcut.
    private(set) var registered = false

    deinit {
        if let ref { UnregisterEventHotKey(ref) }
    }

    init(keyCode: UInt32, modifiers: UInt32, id: UInt32, handler: @escaping () -> Void) {
        self.id = id
        HotKey.handlers[id] = handler
        if !HotKey.installed {
            HotKey.installed = true
            var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            InstallEventHandler(GetApplicationEventTarget(), { _, event, _ in
                var hk = EventHotKeyID()
                GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                                  MemoryLayout<EventHotKeyID>.size, nil, &hk)
                DispatchQueue.main.async { HotKey.handlers[hk.id]?() }
                return noErr
            }, 1, &spec, nil, nil)
        }
        registered = RegisterEventHotKey(keyCode, modifiers, EventHotKeyID(signature: OSType(0x49564C54), id: id),
                                         GetApplicationEventTarget(), 0, &ref) == noErr
    }
}

@MainActor
final class AppController: NSObject, NSApplicationDelegate {
    private var island: IslandController!
    private let clipboard = ClipboardWatcher()
    private let screenshots = ScreenshotWatcher()
    private var hotKey: HotKey?
    private var copyHotKey: HotKey?
    private var ocrHotKey: HotKey?
    private var shotHotKey: HotKey?
    private var statusItem: StatusItemController?

    /// A configurable global shortcut (default ⌃⌥ + key), stored in UserDefaults.
    struct Shortcut {
        let name: String
        let codeKey: String
        let modsKey: String
        let defaultCode: Int
        let id: UInt32

        var keyCode: UInt32 {
            let v = UserDefaults.standard.integer(forKey: codeKey)
            return UInt32(v != 0 ? v : defaultCode)
        }
        var modifiers: UInt32 {
            let v = UserDefaults.standard.integer(forKey: modsKey)
            return UInt32(v != 0 ? v : (controlKey | optionKey))
        }
        var label: String { "\(AppController.modifierFlagsToString(modifiers))\(AppController.keyCodeToString(Int(keyCode)))" }
        var defaultLabel: String { "⌃⌥\(AppController.keyCodeToString(defaultCode))" }
    }

    static let ocrShortcut = Shortcut(name: "OCR", codeKey: "ocrHotKeyCode", modsKey: "ocrHotKeyModifiers",
                                      defaultCode: kVK_ANSI_T, id: 3)
    static let shotShortcut = Shortcut(name: "Screenshot", codeKey: "shotHotKeyCode", modsKey: "shotHotKeyModifiers",
                                       defaultCode: kVK_ANSI_S, id: 4)

    func applicationDidFinishLaunching(_ notification: Notification) {
        removeChromeBridge()
        installMainMenu()
        VaultStore.shared.migrateToFlatLayout()
        VaultModel.shared.start()
        OCRIndexer.shared.start()
        island = IslandController()
        island.onMenu = { [weak self] event in self?.showMenu(event) }
        statusItem = StatusItemController(island: island, clipboard: clipboard, screenshots: screenshots)

        let announce: (Ingest.Summary) -> Void = { [weak self] summary in
            self?.island.showActivity(image: summary.firstImage, title: "Saved",
                                      detail: "\(VaultModel.shared.items.count) in vault")
        }
        clipboard.onSaved = announce
        screenshots.onSaved = announce
        clipboard.start()
        screenshots.start()

        // ⌃⌥V opens / closes the island from anywhere.
        hotKey = HotKey(keyCode: UInt32(kVK_ANSI_V), modifiers: UInt32(controlKey | optionKey), id: 1) { [weak self] in
            self?.island.toggle()
        }
        // ⌃⌥B copies the newest vault image from anywhere.
        copyHotKey = HotKey(keyCode: UInt32(kVK_ANSI_B), modifiers: UInt32(controlKey | optionKey), id: 2) { [weak self] in
            self?.statusItem?.copyLatest()
        }
        // ⌃⌥T captures screen region → OCR → clipboard (configurable)
        register(Self.ocrShortcut)
        // ⌃⌥S takes a screenshot → floating preview (configurable)
        register(Self.shotShortcut)
    }

    private func startScreenOCR() {
        ScreenOCRController.shared.toggle()
    }

    func takeScreenshot() {
        CaptureCenter.shared.takeScreenshot()
    }

    private func register(_ shortcut: Shortcut) {
        let key = HotKey(keyCode: shortcut.keyCode, modifiers: shortcut.modifiers, id: shortcut.id) { [weak self] in
            if shortcut.id == Self.shotShortcut.id { self?.takeScreenshot() } else { self?.startScreenOCR() }
        }
        if shortcut.id == Self.shotShortcut.id { shotHotKey = key } else { ocrHotKey = key }
    }

    private func unregister(_ shortcut: Shortcut) {
        if shortcut.id == Self.shotShortcut.id { shotHotKey = nil } else { ocrHotKey = nil }
    }

    private func hotKey(for shortcut: Shortcut) -> HotKey? {
        shortcut.id == Self.shotShortcut.id ? shotHotKey : ocrHotKey
    }

    /// Accessory apps have no menu bar, so text fields lose ⌘A / ⌘C / ⌘V / ⌘Z.
    /// A hidden main menu restores the standard editing shortcuts.
    private func installMainMenu() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit Image Vault", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        main.addItem(appItem)
        let editItem = NSMenuItem()
        let edit = NSMenu(title: "Edit")
        edit.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        let redo = edit.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        edit.addItem(.separator())
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = edit
        main.addItem(editItem)
        NSApp.mainMenu = main
    }

    private var ocrShortcutLabel: String { Self.ocrShortcut.label }

    func applicationWillTerminate(_ notification: Notification) {
        NowPlaying.shared.stop()
    }

    /// Earlier builds registered a Chrome native-messaging host; the app is standalone now.
    private func removeChromeBridge() {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        for browser in ["Google/Chrome", "Google/Chrome Beta", "Google/Chrome Canary", "Chromium",
                        "BraveSoftware/Brave-Browser", "Microsoft Edge", "Arc/User Data", "Vivaldi"] {
            let file = support.appendingPathComponent(browser).appendingPathComponent("NativeMessagingHosts/com.imagevault.bridge.json")
            try? FileManager.default.removeItem(at: file)
        }
    }

    // MARK: Settings menu (gear in the island, or right-click it)

    private func showMenu(_ event: NSEvent?) {
        let menu = NSMenu()
        menu.autoenablesItems = false
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"
        let bytes = VaultModel.shared.items.reduce(Int64(0)) { $0 + Int64($1.bytes ?? 0) }
        let size = ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
        menu.addItem(disabled("Image Vault \(version)  ·  \(VaultModel.shared.items.count) images  ·  \(size)"))
        menu.addItem(.separator())

        let positionItem = NSMenuItem(title: "Island Position", action: nil, keyEquivalent: "")
        let positions = NSMenu()
        for p in IslandPosition.allCases {
            positions.addItem(toggle("Top \(p.title)", on: island.position == p) { [weak self] in self?.island.position = p })
        }
        positionItem.submenu = positions
        menu.addItem(positionItem)

        let displayItem = NSMenuItem(title: "Display", action: nil, keyEquivalent: "")
        let displays = NSMenu()
        let current = island.screen.map { IslandController.id(of: $0) }
        for screen in NSScreen.screens {
            let id = IslandController.id(of: screen)
            let notch = screen.safeAreaInsets.top > 0 ? "  (notch)" : ""
            displays.addItem(toggle("\(screen.localizedName)\(notch)", on: id == current) { [weak self] in self?.island.displayID = id })
        }
        displayItem.submenu = displays
        menu.addItem(displayItem)
        menu.addItem(toggle("Show Notch", on: island.showNotch) { [weak self] in self?.island.showNotch.toggle() })
        menu.addItem(.separator())

        menu.addItem(toggle("Save Copied Images", on: clipboard.enabled) { [weak self] in self?.clipboard.enabled.toggle() })
        menu.addItem(toggle("Save New Screenshots", on: screenshots.enabled) { [weak self] in self?.screenshots.enabled.toggle() })
        menu.addItem(toggle("Clipboard History", on: ClipboardHistory.shared.enabled) { ClipboardHistory.shared.enabled.toggle() })
        menu.addItem(toggle("Music Controls", on: NowPlaying.shared.enabled) { NowPlaying.shared.enabled.toggle() })
        menu.addItem(toggle("Open at Login", on: SMAppService.mainApp.status == .enabled) { [weak self] in self?.toggleLogin() })
        menu.addItem(.separator())

        // Screenshot + screen OCR
        menu.addItem(action("Take Screenshot  \(Self.shotShortcut.label)") { [weak self] in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { self?.takeScreenshot() }
        })
        menu.addItem(action("Capture Text from Screen  \(ocrShortcutLabel)") { [weak self] in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { self?.startScreenOCR() }
        })
        let cardItem = NSMenuItem(title: "Screenshot Preview", action: nil, keyEquivalent: "")
        let cardMenu = NSMenu()
        for (title, seconds) in [("3 Seconds", 3), ("5 Seconds", 5), ("10 Seconds", 10), ("Until Closed", 0)] {
            cardMenu.addItem(toggle(title, on: CaptureCenter.previewSeconds == seconds) { CaptureCenter.previewSeconds = seconds })
        }
        cardItem.submenu = cardMenu
        menu.addItem(cardItem)
        let shortcutsItem = NSMenuItem(title: "Shortcuts", action: nil, keyEquivalent: "")
        let shortcutsMenu = NSMenu()
        for shortcut in [Self.shotShortcut, Self.ocrShortcut] {
            shortcutsMenu.addItem(disabled("\(shortcut.name): \(shortcut.label)"))
            shortcutsMenu.addItem(action("Change \(shortcut.name) Shortcut…") { [weak self] in self?.changeHotkey(shortcut) })
            shortcutsMenu.addItem(action("Reset to \(shortcut.defaultLabel)") { [weak self] in self?.resetHotkey(shortcut) })
            if hotKey(for: shortcut)?.registered == false {
                shortcutsMenu.addItem(disabled("⚠︎ Taken by another app — pick a different one"))
            }
            shortcutsMenu.addItem(.separator())
        }
        shortcutsMenu.removeItem(at: shortcutsMenu.numberOfItems - 1)
        shortcutsItem.submenu = shortcutsMenu
        menu.addItem(shortcutsItem)

        let languagesItem = NSMenuItem(title: "OCR Languages", action: nil, keyEquivalent: "")
        let languages = NSMenu()
        languages.addItem(disabled("Built in: English, Chinese, Japanese, Korean, Arabic, Russian, European…"))
        languages.addItem(.separator())
        for language in Tesseract.languages {
            let on = Tesseract.enabled.contains(language.code)
            languages.addItem(toggle(language.name, on: on) {
                var set = Tesseract.enabled
                if on { set.removeAll { $0 == language.code } } else { set.append(language.code) }
                Tesseract.enabled = set
                OCRIndexer.shared.reindexAll()
            })
        }
        if !Tesseract.available {
            languages.addItem(.separator())
            languages.addItem(disabled("Extra languages engine missing from this build"))
        }
        languagesItem.submenu = languages
        menu.addItem(languagesItem)
        menu.addItem(.separator())
        menu.addItem(action("Open Vault Folder") { NSWorkspace.shared.open(VaultStore.shared.imagesURL) })
        menu.addItem(action("Copy Latest Image  ⌃⌥B") { [weak self] in self?.statusItem?.copyLatest() })
        menu.addItem(disabled("Open / close: hover the island or ⌃⌥V"))
        menu.addItem(.separator())
        menu.addItem(action("Quit Image Vault") { NSApp.terminate(nil) })

        island.menuOpen = true
        let view = island.window.contentView!
        if let event { NSMenu.popUpContextMenu(menu, with: event, for: view) }
        else {
            let p = view.convert(island.window.convertPoint(fromScreen: NSEvent.mouseLocation), from: nil)
            menu.popUp(positioning: nil, at: p, in: view)
        }
        island.menuOpen = false
        island.pointerMoved()
    }

    private func disabled(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    private func action(_ title: String, _ run: @escaping () -> Void) -> NSMenuItem {
        ClosureMenuItem(title: title, run: run)
    }

    private func toggle(_ title: String, on: Bool, _ run: @escaping () -> Void) -> NSMenuItem {
        let item = ClosureMenuItem(title: title, run: run)
        item.state = on ? .on : .off
        return item
    }

    private func toggleLogin() {
        do {
            if SMAppService.mainApp.status == .enabled { try SMAppService.mainApp.unregister() }
            else { try SMAppService.mainApp.register() }
        } catch {
            island.flash("Couldn't change login item")
        }
    }

    private func changeHotkey(_ shortcut: Shortcut) {
        // The current shortcut would otherwise fire instead of being recorded.
        unregister(shortcut)
        NSApp.activate(ignoringOtherApps: true)
        defer { if hotKey(for: shortcut) == nil { register(shortcut) } }
        let alert = NSAlert()
        alert.messageText = "Set \(shortcut.name) Shortcut"
        alert.informativeText = "Press the key combination to use (e.g. \(shortcut.defaultLabel)). Escape to cancel."
        alert.addButton(withTitle: "Cancel")
        alert.alertStyle = .informational
        
        // Create a custom view with a key capture field
        let captureView = HotkeyCaptureView()
        captureView.onCapture = { [weak self] keyCode, modifiers in
            guard let self else { return }
            UserDefaults.standard.set(Int(keyCode), forKey: shortcut.codeKey)
            UserDefaults.standard.set(Int(modifiers), forKey: shortcut.modsKey)
            self.unregister(shortcut)
            self.register(shortcut)
            if self.hotKey(for: shortcut)?.registered == false {
                self.island.showActivity(image: nil, title: "Shortcut taken", detail: "\(shortcut.label) is used by another app")
            } else {
                self.island.showActivity(image: nil, title: "\(shortcut.name) shortcut set", detail: shortcut.label)
            }
        }
        captureView.frame = NSRect(x: 0, y: 0, width: 260, height: 50)
        alert.accessoryView = captureView
        
        // Focus the capture view
        DispatchQueue.main.async {
            captureView.startCapture()
        }
        
        alert.runModal()
    }

    private func resetHotkey(_ shortcut: Shortcut) {
        UserDefaults.standard.removeObject(forKey: shortcut.codeKey)
        UserDefaults.standard.removeObject(forKey: shortcut.modsKey)
        unregister(shortcut)
        register(shortcut)
        island.flash("\(shortcut.name) shortcut reset to \(shortcut.defaultLabel)")
    }

    // Helper to convert modifier flags to display string
    nonisolated static func modifierFlagsToString(_ flags: UInt32) -> String {
        var parts: [String] = []
        if flags & UInt32(controlKey) != 0 { parts.append("⌃") }
        if flags & UInt32(optionKey) != 0 { parts.append("⌥") }
        if flags & UInt32(shiftKey) != 0 { parts.append("⇧") }
        if flags & UInt32(cmdKey) != 0 { parts.append("⌘") }
        return parts.joined()
    }

    // Helper to convert key code to display string
    nonisolated static func keyCodeToString(_ keyCode: Int) -> String {
        switch keyCode {
        case kVK_ANSI_A: return "A"
        case kVK_ANSI_B: return "B"
        case kVK_ANSI_C: return "C"
        case kVK_ANSI_D: return "D"
        case kVK_ANSI_E: return "E"
        case kVK_ANSI_F: return "F"
        case kVK_ANSI_G: return "G"
        case kVK_ANSI_H: return "H"
        case kVK_ANSI_I: return "I"
        case kVK_ANSI_J: return "J"
        case kVK_ANSI_K: return "K"
        case kVK_ANSI_L: return "L"
        case kVK_ANSI_M: return "M"
        case kVK_ANSI_N: return "N"
        case kVK_ANSI_O: return "O"
        case kVK_ANSI_P: return "P"
        case kVK_ANSI_Q: return "Q"
        case kVK_ANSI_R: return "R"
        case kVK_ANSI_S: return "S"
        case kVK_ANSI_T: return "T"
        case kVK_ANSI_U: return "U"
        case kVK_ANSI_V: return "V"
        case kVK_ANSI_W: return "W"
        case kVK_ANSI_X: return "X"
        case kVK_ANSI_Y: return "Y"
        case kVK_ANSI_Z: return "Z"
        case kVK_ANSI_0: return "0"
        case kVK_ANSI_1: return "1"
        case kVK_ANSI_2: return "2"
        case kVK_ANSI_3: return "3"
        case kVK_ANSI_4: return "4"
        case kVK_ANSI_5: return "5"
        case kVK_ANSI_6: return "6"
        case kVK_ANSI_7: return "7"
        case kVK_ANSI_8: return "8"
        case kVK_ANSI_9: return "9"
        case kVK_Space: return "Space"
        case kVK_Tab: return "Tab"
        case kVK_Return: return "Return"
        case kVK_Escape: return "Esc"
        case kVK_Delete: return "Delete"
        case kVK_LeftArrow: return "←"
        case kVK_RightArrow: return "→"
        case kVK_UpArrow: return "↑"
        case kVK_DownArrow: return "↓"
        case kVK_F1: return "F1"
        case kVK_F2: return "F2"
        case kVK_F3: return "F3"
        case kVK_F4: return "F4"
        case kVK_F5: return "F5"
        case kVK_F6: return "F6"
        case kVK_F7: return "F7"
        case kVK_F8: return "F8"
        case kVK_F9: return "F9"
        case kVK_F10: return "F10"
        case kVK_F11: return "F11"
        case kVK_F12: return "F12"
        default: return "Key(\(keyCode))"
        }
    }
}

/// View that captures a key combination for hotkey configuration
final class HotkeyCaptureView: NSView {
    var onCapture: ((UInt32, UInt32) -> Void)?
    private var captured = false
    private let label = NSTextField(labelWithString: "Press key combination…")
    
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        layer?.cornerRadius = 8
        
        label.font = .systemFont(ofSize: 13, weight: .medium)
        label.textColor = .labelColor
        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: centerXAnchor),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            widthAnchor.constraint(greaterThanOrEqualToConstant: 200),
            heightAnchor.constraint(equalToConstant: 50)
        ])
    }
    
    required init?(coder: NSCoder) { fatalError() }
    
    func startCapture() {
        captured = false
        label.stringValue = "Press key combination…"
        window?.makeFirstResponder(self)
    }
    
    override var acceptsFirstResponder: Bool { true }

    /// ⌘-combos arrive as key equivalents (the alert's buttons would eat them).
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard window?.firstResponder === self, event.type == .keyDown else { return false }
        keyDown(with: event)
        return true
    }
    
    override func keyDown(with event: NSEvent) {
        guard !captured else { return }
        
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let keyCode = event.keyCode
        
        // Ignore modifier-only keys
        if keyCode == kVK_Control || keyCode == kVK_Option || keyCode == kVK_Shift || keyCode == kVK_Command {
            return
        }
        
        // Ignore Escape (cancel)
        if keyCode == kVK_Escape {
            NSApp.stopModal()   // closing the alert window alone leaves runModal() hanging
            return
        }
        
        var carbonMods: UInt32 = 0
        if modifiers.contains(.control) { carbonMods |= UInt32(controlKey) }
        if modifiers.contains(.option) { carbonMods |= UInt32(optionKey) }
        if modifiers.contains(.shift) { carbonMods |= UInt32(shiftKey) }
        if modifiers.contains(.command) { carbonMods |= UInt32(cmdKey) }
        
        // Require at least one modifier for global hotkeys
        if carbonMods == 0 {
            label.stringValue = "Please include a modifier (⌃ ⌥ ⇧ ⌘)"
            return
        }
        
        captured = true
        label.stringValue = "\(AppController.modifierFlagsToString(carbonMods))\(AppController.keyCodeToString(Int(keyCode)))"
        
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
            NSApp.stopModal()
            self?.onCapture?(UInt32(keyCode), carbonMods)
        }
    }
    
    override func flagsChanged(with event: NSEvent) {
        // Show current modifiers while holding
        if !captured {
            let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            var carbonMods: UInt32 = 0
            if modifiers.contains(.control) { carbonMods |= UInt32(controlKey) }
            if modifiers.contains(.option) { carbonMods |= UInt32(optionKey) }
            if modifiers.contains(.shift) { carbonMods |= UInt32(shiftKey) }
            if modifiers.contains(.command) { carbonMods |= UInt32(cmdKey) }
            if carbonMods != 0 {
                label.stringValue = "\(AppController.modifierFlagsToString(carbonMods)) + key…"
            }
        }
    }
}

final class ClosureMenuItem: NSMenuItem {
    private let run: () -> Void
    init(title: String, key: String = "", modifiers: NSEvent.ModifierFlags = .command, run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(fire), keyEquivalent: key)
        keyEquivalentModifierMask = modifiers
        target = self
    }
    required init(coder: NSCoder) { fatalError() }
    @objc private func fire() { run() }
}
