# Image Vault

A pure black-and-white image vault that lives in a Dynamic-Island-style notch at the top of the screen. Drop images on it, copy them back out as real image data, or drag them into any app as real files.

| Folder | Platform |
| --- | --- |
| [`mac/`](mac/README.md) | macOS app in Swift (SwiftUI + AppKit). This is the reference design. |
| [`windows/`](windows/README.md) | Windows app in Rust (Win32 + Direct2D + DirectComposition), a native port of the Mac app |

Both apps use `vault.json`. The Mac app keeps every image in one `images/` folder (since 2026-10-29); the Windows app still uses `images/<id>/<name>`.

The island opens when the pointer touches the very top edge of the screen above it (or with Ctrl+Alt+V / ⌃⌥V).

Project history, decisions, platform findings and the current state of both apps are in [`KNOWLEDGEBASE.md`](KNOWLEDGEBASE.md).
