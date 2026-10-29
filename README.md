# Image Vault

A pure black-and-white image vault that lives in a Dynamic-Island-style notch at the top of the screen. Drop images on it, copy them back out as real image data, or drag them into any app as real files.

| Folder | Platform |
| --- | --- |
| [`mac/`](mac/README.md) | macOS app in Swift (SwiftUI + AppKit). This is the reference design. |
| [`windows/`](windows/README.md) | Windows app in Rust (Win32 + Direct2D + DirectComposition), a native port of the Mac app |

Images are plain files in an `images/` folder. The Mac app indexes them in SQLite (`vault.sqlite`, with full-text search over names and OCR text, since 2026-10-30) and keeps every image in one folder; the Windows app uses `vault.json`.

The island opens when the pointer touches the very top edge of the screen above it (or with Ctrl+Alt+V / ⌃⌥V).

Project history, decisions, platform findings and the current state of both apps are in [`KNOWLEDGEBASE.md`](KNOWLEDGEBASE.md).
