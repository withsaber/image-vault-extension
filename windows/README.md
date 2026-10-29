# Image Vault for Windows

A native Windows port of the Mac app, written in Rust on Win32, Direct2D, DirectComposition, DirectWrite and WIC. It has no runtime or framework dependencies and ships as one ~3 MB `.exe`. At rest it uses no CPU, and Windows trims its working set to under 1 MB.

It has the same features, layout and motion as the Mac app (see [../mac/README.md](../mac/README.md)): the island at the top of the screen, the tray that opens when the pointer touches the top screen edge above the island, the Live Activity, the drag hint and drop target, the Vault and Clipboard tabs, search and sort, file-manager selection with lasso, drag-out of real files, copy as real image data, preview, Copy Text (OCR) with background indexing, Now Playing with previous / play-pause / next and a display-only waveform, screen OCR, screenshots with floating previews, pins and the editor, and the notification-area icon.

## Windows equivalents

| Mac | Windows |
| --- | --- |
| ⌃⌥V / ⌃⌥B / ⌃⌥T / ⌃⌥S | Ctrl+Alt+V / B / T / S (T and S can be changed under Settings → Shortcuts) |
| ⌘C, ⌘A, ⌘Z, ⌘T | Ctrl+C, Ctrl+A, Ctrl+Z, Ctrl+T |
| Menu bar item | Notification-area icon |
| Reveal in Finder | Show in Explorer |
| Vision OCR | Windows OCR (`Windows.Media.Ocr`) with the same layout pass |
| MediaRemote | System Media Transport Controls (Spotify, browsers, Media Player…) |
| Screen Recording permission | Not needed |
| `~/Library/Application Support/ImageVault` | `%LOCALAPPDATA%\ImageVault` (same `vault.json` format) |

SF Symbols are Apple-only, so the icons are redrawn as vector paths in the same style. Windows has no hardware notch, so the island always draws its own.

## Known issue

Dragging a screenshot's floating preview into WhatsApp, an AI chat or a web upload field doesn't drop. The likely cause is that the preview doesn't capture the mouse, so a quick drag leaves the small window before the drag starts. The proposed fix is `SetCapture` on press in `capture.rs`. It isn't applied yet; see `KNOWLEDGEBASE.md` §7.

## OCR languages (Bangla, Hindi, Tamil…)

Windows OCR reads the languages installed on your PC. For the extra scripts the app uses Tesseract, like the Mac build. Install it once:

```bash
winget install UB-Mannheim.TesseractOCR
```

`build.cmd` copies the language data (`ben`, `hin`, `urd`, `tam`, `tel`, `mar`, `nep`, `guj`, `pan`, `mal`, `kan`, `sin`) next to the exe in `dist\tesseract\tessdata`. You can also put `tesseract.exe` itself in `dist\tesseract\`.

## Build

You need Rust (`x86_64-pc-windows-gnu` with MinGW, or MSVC).

```bash
build.cmd
```

This produces `dist\ImageVault.exe`. Copy it anywhere and run it. Settings → Open at Login starts it with Windows.

## Developer aids

```bash
ImageVault.exe --snapshot <dir>           # render every island state off-screen to PNGs
ImageVault.exe --capture-demo <img> <dir> # annotate an image with every editor tool, open preview + editor
ImageVault.exe --ocr <file>               # print the OCR text
ImageVault.exe --nowplaying               # print what Windows reports as Now Playing
ImageVault.exe --quit                     # quit the running instance
```

Set `IMAGEVAULT_ROOT` to point the app at a throwaway vault, and set `IV_TRACE=1` to write `trace.log` there.

## Source map

| File | Mac counterpart |
| --- | --- |
| `island.rs`, `views.rs` | Island.swift (window, states, header, search, bulk bar, drop target, clipboard grid) |
| `tray.rs` | Tray.swift + Tile.swift |
| `vault.rs`, `imaging.rs`, `thumbs.rs` | VaultStore / VaultModel / Imaging |
| `ingest.rs`, `clipboard.rs`, `cliphistory.rs`, `dragdrop.rs` | Ingest.swift, ClipboardHistory.swift |
| `ocr.rs`, `screenocr.rs` | OCR.swift, ScreenOCR.swift |
| `capture.rs`, `editor.rs` | Capture.swift, Editor.swift |
| `nowplaying.rs` | NowPlaying.swift |
| `app.rs`, `hotkeys.rs`, `statusitem.rs`, `settings.rs` | AppController.swift, StatusItem.swift |
| `anim.rs` | SwiftUI springs, CASpringAnimation, CAMediaTimingFunction |
| `gfx.rs`, `icons.rs`, `textfield.rs`, `menu.rs`, `tooltip.rs`, `ui.rs` | Rendering, SF Symbol shapes, text editing, dark menus, help tags, run loop |
