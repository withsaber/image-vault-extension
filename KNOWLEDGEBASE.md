# Image Vault — Project Knowledge Base

Everything decided, built, learned and fixed across this project, in one place.
Last updated: **2026-10-30**. See [Current state](#9-current-state-as-of-2026-10-29).

---

## Contents

1. [What Image Vault is](#1-what-image-vault-is)
2. [Timeline](#2-timeline)
3. [Your requirements and preferences](#3-your-requirements-and-preferences)
4. [Repository layout](#4-repository-layout)
5. [Chrome extension (retired)](#5-chrome-extension-retired)
6. [Mac app (main product)](#6-mac-app-main-product)
7. [Windows app](#7-windows-app)
8. [Platform findings and lessons](#8-platform-findings-and-lessons)
9. [Current state (as of 2026-10-22)](#9-current-state-as-of-2026-10-22)
10. [Bug log](#10-bug-log)
11. [Building, sharing and tooling](#11-building-sharing-and-tooling)

---

## 1. What Image Vault is

A personal image vault in a pure black-and-white design. You save images from anywhere (drag in, copy, screenshot) and get them back out by copy-paste or drag-out into any app (Figma, WhatsApp, Slack, Photoshop, Finder).

It now exists as three things:

| Product | Status |
| --- | --- |
| **Mac app**: a Dynamic Island in the notch, NotchNook-style | **Main product.** In daily use. |
| **Windows app**: a native Rust port of the Mac app (Win32, Direct2D) | **Your rewrite.** One ~3 MB `.exe` with full feature parity. Last updated 2026-10-22. |
| **Chrome extension**: the original version | **Retired.** You said you won't use it. It isn't in this folder; the code is still in `~/Downloads/image-vault-extension`. |

---

## 2. Timeline

| Phase | What happened |
| --- | --- |
| **v1** (before this conversation) | A Chrome extension with an always-visible drawer, an indigo theme, and copy as URL text. Backup: `~/Downloads/image-vault-extension-v1-backup.zip`. |
| **Extension v2.0** | Monochrome rebuild. The drop zone appears only while dragging. Real PNG clipboard copy, file-manager selection, a virtual grid, image bytes stored in IndexedDB. |
| **Drag-out saga** | Drag-out into Figma, WhatsApp and Gemini failed. Root causes: Chrome passes only text between tabs, and CDNs send AVIF. Fixed with `dragshim.js` (real files replayed into pages) and format normalization. |
| **Clipboard saga** | WhatsApp's permissions policy blocks the clipboard in iframes *and* in its own page. Fixed with a host-page relay plus a `copy.html` helper window. Extension reached v2.1–2.2. |
| **Mac app v1.0** | Native Swift app with an edge tab, panel, drop zone, clipboard and screenshot auto-save, and Chrome sync over native messaging. |
| **Mac app v2.0: Dynamic Island** | Rebuilt as a NotchNook-style island. Positions: top-left, top-center, top-right; you choose the display. Collapses when the pointer leaves, and shows a Live Activity on save. Chrome sync removed. |
| **Sharing question** | Which file to send to someone (see §11). |
| **Windows app (first port)** | C#/WPF port of island v2.0 with SwiftUI's spring maths, published as portable x64/ARM64 `.exe` files. Later replaced by your Rust rewrite (see below). |
| **Outside this conversation** | Search and sort, a menu bar item, Now Playing (music controls), OCR (Copy Text), screen OCR v1, the `ObjCExceptionCatcher` shim, and the Mac app at v2.2.0. |
| **2026-10-19 fix pass** | Screen OCR rewrite, Bangla via Tesseract, auto-OCR for search, player fixes, waveform scrubber, smaller icons, search fixes. Built and installed. |
| **Outside this conversation (2026-10-19)** | Clipboard tab, screenshots with floating previews, pins and the editor, Show Notch, a stable local signing identity (`setup-signing.sh`), and the image preview fix. |
| **Windows app rewritten in Rust** (you, ~2026-10-20) | You replaced the C#/WPF port with a native Rust app (Win32, Direct2D, DirectComposition, DirectWrite, WIC): one ~3 MB `.exe`, under 1 MB of memory at rest, the same features as the Mac app. The project moved to `~/Desktop/best works/image-vault-extension`. |
| **Size question** (2026-10-22) | Why the Mac app is heavier than Windows (see §8). You chose to **keep the Mac app as it is**: no slimming. |
| **2026-10-22 updates** | Music strip: previous / play-pause / next buttons, and the waveform is display-only (no seeking). The island opens only when the pointer touches the very top screen edge. Done on Mac, then ported to Windows. |
| **2026-10-29 second pass (Mac only)** | Screenshots auto-save to the vault (and later edits update that item), the 2×2 hover grid from your sketch, other images fade while a folder is open, the opening jitter fixed, and copying a clipboard card no longer moves it to the top. |
| **2026-10-29 feedback pass (Mac only)** | One images folder (no per-image folders), folders/groups with the fly-out animation, live GIFs, SVG badges + Copy SVG Code, SVG code preview in Clipboard, Edit on every image, screenshot auto-copy + card timer (3/5/10 s/until closed) that closes after you use it, copy overlay animation, expandable clipboard cards, icon-only tabs, new select-all icon, Group replaces Text in the bulk bar. Not ported to Windows. |
| **2026-10-30 storage pass (Mac only)** | The index moved from `vault.json` to SQLite (`vault.sqlite`) with FTS5 trigram search; OCR results save off the main thread without reloading the grid; thumbnails decode at 256 px with a 96 MB memory budget and a disk cache in `thumbs/`. `vault.json` is imported once and kept as `vault.json.pre-sqlite`. See §6.3. |

---

## 3. Your requirements and preferences

**Visual design**
- Pure black (`#000`) and white, plus off-white `#E5E5E5`, with hairline borders and sharp typography.
- Fluid motion: about 200–300 ms, with springs. Subtle hover scale (1.02–1.05), micro-feedback (check marks, pulses, snaps), smooth entrances and exits.

**The island**
- It must look and move like NotchNook's Dynamic Island ("100% the same"), but only for image storage. It's a recreation, not their code or artwork.
- Collapses when the pointer leaves (chosen option).
- Placement: top-left, top-center or top-right, on any chosen display.
- Resting state is a plain black notch. A Live Activity appears on save.

**Behavior**
- The drop zone is visible only while dragging; never a persistent drop target.
- Copy must put **real image data** on the clipboard, never a URL.
- File-manager selection: click, Shift, Cmd/Ctrl, lasso, select all.
- No bulk clipboard copy in the browser. Multi-select works via drag and drop and file behavior.
- Image storage keeps the actual bytes (chosen over URL-only).

**Platforms**
- Mac first; the Chrome extension is no longer used.
- Windows: your native Rust port, delivered as one portable `.exe`.
- ~~Change only the Mac app unless you explicitly ask for Windows.~~ **Replaced 2026-10-30:** every change lands on both platforms. Build it on one side first (usually the Mac), then port it to the other; record anything that can't be ported yet as a parity gap in §9. See `CLAUDE.md`.
- Keep the Mac app's size and memory as they are (no slimming, 2026-10-22).

**Opening the island (2026-10-22)**
- Only when the pointer touches the very top pixel row of the screen, within the notch's width. Passing near the notch, for example on the way to a browser's tab bar, must not open it.

**OCR (2026-10-19 request)**
- Works like Shotr. Uses your crosshair cursor (white plus with a soft gray dot). No full-screen guide lines, and the overlay never takes over the screen.
- Works on every monitor. Esc must cancel. The shortcut must be configurable.
- Supports Bangla and other languages.
- Search must find text inside images, so every added image is OCR'd automatically.

**Player**
- Show the real song artwork, not the browser icon.
- The waveform bar style from your reference image replaces the dancing bars.
- Looping must keep the correct position.
- Previous, play/pause and next buttons. The progress waveform is **untouchable**: no clicking or dragging to seek (2026-10-22).

**Working style:** you welcome clarifying questions ("ask me 100 times").

---

## 4. Repository layout

```
~/Desktop/best works/image-vault-extension/
├── KNOWLEDGEBASE.md            ← this file
├── README.md                   Overview of both apps
│
├── mac/                        Mac app (Swift, SwiftUI + AppKit) — the reference design
│   ├── Package.swift           Targets: ImageVault (exe) + ObjCExceptionCatcher (ObjC shim)
│   ├── build.sh                Release build → /Applications/Image Vault.app (signed with the local identity)
│   ├── setup-signing.sh        One-time: creates the stable "Image Vault Local Signing" certificate
│   ├── README.md
│   ├── Resources/              Info.plist (v2.2.0, LSUIElement), AppIcon.icns
│   ├── Helpers/ivmr.c          Now-playing artwork helper (loaded into /usr/bin/perl)
│   ├── Vendor/tesseract/       Bundled Tesseract 5.5.3: bin/, lib/ (relinked), tessdata/ (13 langs)
│   └── Sources/ImageVault/     (see §6.2)
│
├── windows/                    Windows app (Rust, Win32 + Direct2D) — v2.2.0
│   ├── Cargo.toml / build.rs   Crate "imagevault", binary ImageVault.exe
│   ├── build.cmd               Release build on Windows → dist\ImageVault.exe (+ tessdata)
│   ├── README.md
│   ├── res/                    AppIcon.ico, app.manifest, app.rc
│   ├── examples/               probe.rs, snap.rs (dev helpers)
│   ├── dist/                   ImageVault.exe (~3 MB) + tesseract/tessdata
│   └── src/                    (see §7)
│
└── windows-rs/                 Old Cargo build cache only (no source)
```

---

## 5. Chrome extension (retired)

The extension is kept for reference in `~/Downloads/image-vault-extension` (it isn't part of this folder). You said you won't use it, and the Mac app no longer syncs with it.

**Architecture**
- **Content script (`content.js`)**: a Shadow-DOM edge tab, a lazy iframe panel, and a drop zone that appears only during drags. The panel hovers to peek and clicks to pin.
- **Gallery (`gallery.html`)**:
  - Runs inside an extension iframe, so page CSP doesn't apply. Also works as a full page.
  - Virtual grid and file-manager selection.
  - Native image drag-out, a PNG clipboard, preview, download and delete with undo.
- **Storage**: metadata in `chrome.storage.local` (`vaultImages`), bytes in IndexedDB (`{full, thumb}`), and tombstones for sync.
- **Background**:
  - Fetches with host permissions, which bypasses CORS.
  - Normalizes formats: AVIF/WebP become JPEG or PNG.
  - Dedupes by SHA-256 and runs garbage collection.
  - Handles the toolbar toggle (Alt+Shift+V) and the drag bridge.
- **`dragshim.js`**:
  - Runs at `document_start` in all frames and intercepts drags carrying the vault marker or the broadcast flag.
  - Replays them to the element under the cursor with real `File` objects.
  - Guards against intercepting its own replays (`replaying` flag).
- **Clipboard relay**: when a page blocks the clipboard, the panel asks the host page's content script to write it. If that fails too, `copy.html` opens as a helper window.

---

## 6. Mac app (main product)

### 6.1 Island specification

**States**: `collapsed` → `peek` (hover breath) → `expanded` (tray), plus `activity` (Live Activity), `dragHint` and `dragOver`.

**Sizes** (body, excluding the flared "ears"):

| State | Size |
| --- | --- |
| collapsed | Notch width × **10 px** (hidden when Show Notch is off) |
| peek | Notch +18 × 15 |
| activity | Notch +176 × +2 |
| dragHint | max(notch+170, 340) × (notch height + 46) |
| expanded / dragOver | min(660, screen−60) × (band + 8 + 236) |

**Corner radii** (top ears / bottom):

| State | Top / bottom |
| --- | --- |
| collapsed, peek, activity | 6 / min(14, h × 0.45) |
| dragHint | 10 / 20 |
| expanded, dragOver | 16 / 30 |

**Springs** (SwiftUI response / damping):

| Motion | Response / damping |
| --- | --- |
| Open | 0.42 / 0.74 (slight overshoot) |
| Close | 0.36 / 0.90 |
| Content in | 0.38 / 0.86, with a 0.07 s delay |
| Content out | easeOut 0.12 s |
| Hover | 0.26 / 0.62 |

**Behavior**
- **Opening (2026-10-22):** the pointer must touch the very top edge of the screen (a 2–3 pt strip at the top pixel row), within the notch's width (`triggerZone` in `Island.swift`). Above a Live Activity, its full width counts. After the 0.09 s "breath" the island opens; while it's opening, the larger notch-sized `hoverZone` keeps it open. Before this change, anywhere in the notch's menu-bar-tall area opened it, so moving toward a browser's tab bar opened it by accident.
- Leaving closes it **immediately** (hotkey/menu opens keep a 1.6 s grace).
- Settings → **Show Notch** hides the resting notch.
- The Live Activity lasts 2.4 s.
- Clicks outside the shape go to the app underneath (`ignoresMouseEvents` follows the pointer).
- Opening and dropping trigger a haptic tap.
- The window sits at `mainMenu + 3` level, joins all Spaces and works over full-screen apps.

### 6.2 Source files (`mac/Sources/ImageVault/`)

| File | Role |
| --- | --- |
| `main.swift` | Entry point. Developer flags: `--snapshot <dir>`, `--ocr <file>`, `--nowplaying` |
| `AppController.swift` | App delegate, Carbon hotkeys, settings menu, OCR shortcut capture, hidden Edit menu |
| `Island.swift` | `IslandModel`, `NotchShape`, `IslandView` (SwiftUI), header, search bar, bulk bar, `NowPlayingBar`, `TransportButton`, `WaveformProgress`, `IslandController` (window, hover, drag and drop, activity) |
| `Tray.swift` | `TrayController` (NSCollectionView, diffable data source, filter and sort, copy, Copy Text, right-click menu, drag-out), `PreviewView`, `IconButton` |
| `Tile.swift` | Tile layers: hover lift, selection ring and tuck, copy and Copy Text buttons, meta strip |
| `VaultStore.swift` | `VaultItem` / `VaultGroup`, the SQLite index (`vault.sqlite`): add/dedupe/delete/restore, folders, OCR text, full-text search, the one-time `vault.json` import |
| `VaultModel.swift` | UI model (reloads on `vaultStoreChanged`), undo, `ClipboardWatcher`, `ScreenshotWatcher` |
| `Ingest.swift` | Reads drops and pasteboards (files → promises → image data → URLs); `Clipboard.copy` / `copyText` |
| `Imaging.swift` | Format normalization, thumbnails, PNG for the clipboard |
| `OCR.swift` | Vision OCR with layout, **Tesseract** fallback, `OCRIndexer` (auto-OCR) |
| `ScreenOCR.swift` | Shotr-style screen text capture (per-display overlays, ScreenCaptureKit) |
| `NowPlaying.swift` | MediaRemote via `osascript`; artwork via the perl helper; seek |
| `StatusItem.swift` | Menu bar item: open, copy latest, recent, toggles |
| `Snapshot.swift` | Off-screen PNG render of every island state, for design checks |
| `Theme.swift` | Tokens, `FloatingPanel` |
| `ObjCExceptionCatcher/` | ObjC shim so Swift survives AppKit file-promise exceptions |

### 6.3 Data

- **Index (since 2026-10-30):** `~/Library/Application Support/ImageVault/vault.sqlite` (WAL mode), replacing `vault.json`, which was rewritten whole on every change. Tables: `items` (one row per image, same fields as before), `groups` + `group_items` (folder membership and order), `meta`, and `items_fts`, an FTS5 **trigram** index over name, domain, source, page title, URLs and OCR text, kept in sync by triggers.
  - **Why:** measured at 10,000 images, an OCR save went from ~113 ms on the main thread (decode + re-encode + rewrite the whole JSON, then reload the grid) to 0.14 ms off it; search from ~18 ms per keystroke to ~1 ms; loading the grid takes ~5 ms.
  - **Search:** `VaultStore.search` uses FTS for 3+ characters (case-insensitive substring, any script, Bangla included) and a `LIKE` scan for shorter needles. The UI model never loads OCR text; the tray caches each query's matching ids.
  - **Changes:** writes bump `VaultStore.generation` and post `vaultStoreChanged` once per run loop; `VaultModel` reloads only if the generation moved. OCR saves post `vaultOCRChanged` instead, which only re-filters an active search. The 1 s file-polling timer and the flock (for the retired Chrome bridge) are gone.
  - **Rule:** never insert into `items` with `OR REPLACE`: SQLite skips delete triggers for replaced rows, which would leave stale search entries. Edits use `UPDATE`.
  - **Migration:** at first launch, `vault.json` is imported in one transaction and renamed `vault.json.pre-sqlite` (a corrupt one is copied to `vault.corrupt-<time>.json`). Tombstones were dropped (Chrome sync is retired).
  - **Thumbnails:** decoded at 256 px (tiles are ~96 pt; was 420 px), memory cache capped at 96 MB by bytes (was 600 images, up to ~400 MB), and saved to `thumbs/<id>@<px>-<hash8>.jpg|png` so a relaunch doesn't decode every original. The content hash in the name makes edited images regenerate; deleting or editing an image removes its thumbnails.
- **Location**: `~/Library/Application Support/ImageVault/vault.sqlite` plus `images/<name>`: **one folder for every image** since 2026-10-29, names kept unique ("Shot.png", "Shot 2.png"). Override with the `IMAGEVAULT_ROOT` environment variable.
- **Migration:** `VaultStore.migrateToFlatLayout()` runs at launch and moves old `images/<id>/<name>` files into `images/`, renaming on clashes and deleting the empty per-image folders. `fileURL(for:)` still falls back to the old path if a file wasn't moved. A pre-migration copy of your vault is at `~/Library/Application Support/ImageVault-backup-2026-10-29`.
- **Windows (since 2026-10-30):** the same design in `windows/src/vault.rs`: `%LOCALAPPDATA%\ImageVault\vault.sqlite` (rusqlite with bundled SQLite), identical tables, FTS5 trigram index and triggers, the one-time `vault.json` import (with its `groups`), the flat `images\<name>` layout with the same migration, and `thumbs\<id>@<px>-<hash8>.jpg|png` with a 96 MB memory budget (`thumbs.rs`). Writes bump a generation and post one change to the UI thread (`vault::set_change_hook`); OCR saves run on the indexer thread and only re-filter an active search.
- **Groups (folders):** the `groups` and `group_items` tables (before 2026-10-30: an optional `groups` array in `vault.json`, `{id: "grp_…", name, itemIDs, created}`, imported once on both platforms). Membership only; files stay in `images/`. An image belongs to at most one group. `VaultModel.groups` / `groupOf` mirror it. Deleting an image keeps its id in the group until the undo window ends (`purgeFiles` prunes it), so ⌘Z puts it back in its folder.
- **`VaultItem` fields**: id, name, mime, bytes, width, height, hash (first 16 bytes of SHA-256, hex), url, pageTitle, pageUrl, domain, timestamp (ms), source, `ocrText`, `ocrVersion`, `ocrLanguages`.
- **Formats**: AVIF, WebP, HEIC, TIFF and BMP are re-encoded on save. Opaque images become JPEG (quality 0.94), the rest PNG.

### 6.4 Clipboard and drag-out

- **Copying one image** puts image data on the clipboard (PNG, TIFF, plus the original JPEG / GIF / SVG bytes so animation and vectors survive in apps that take them).
- **SVG images:** an "SVG" badge; the third hover button (</>) and the menu's "Copy SVG Code" put the SVG source on the clipboard as plain text (no marker, so it shows up in the Clipboard tab with a preview). SVGs show whole (aspect fit). Copied/dropped SVG data (`public.svg-image`) is accepted; detection tolerates a BOM, XML prolog, DOCTYPE or comments.
- **GIFs:** kept byte-for-byte and play live in the grid (`GIFFrames` decodes up to 400 frames at ≤240 px off the main thread; a discrete `contents` keyframe animation on the tile).
- **Hover grid (from your sketch, 2026-10-29):** hovering a tile darkens it and shows a cross dividing it into four actions, centred in each quarter: **select** (top-left; a circle, a filled check when selected), **copy** (top-right), **edit** (bottom-left), **copy text / OCR** (bottom-right; **</>** copy SVG code for SVGs). The buttons pop in one after another; the one under the pointer turns white. Only the buttons are clickable, so the rest of the tile still selects and drags. A selected tile shows a small corner check when not hovered. The name/size shade is gone (the tooltip has them).
- **Edit:** every tile's pencil button (and "Edit Image" in the menu) opens the screenshot editor on that image (`ImageEditing.open`). Save writes back over the original (`VaultStore.replaceData`); GIFs and SVGs save as a new copy so the original isn't flattened.
- **Copying several** puts real file URLs on the clipboard.
- **Our own writes carry the marker** `com.imagevault.internal`, so the clipboard watcher ignores them.
- **Drag-out**: every tile is a real file URL, so multi-select delivers every image.
- **Auto-save** never stores:
  - Concealed, transient or auto-generated pasteboard content (password managers).
  - File copies (`fileURL`).
  - Our own writes.

### 6.5 Shortcuts

| Shortcut | Action |
| --- | --- |
| ⌃⌥V | Toggle the island |
| ⌃⌥B | Copy the latest image |
| ⌃⌥T (configurable) | Screen OCR |
| In the tray | ⌘C, ⌘A, ⌫, ⌘Z, Space, arrows, `/` for search, Esc |

### 6.6 OCR (2026-10-19 design)

**Engines**
- **Apple Vision** (revision 3, accurate, automatic language detection) reads about 30 languages. These include English, Chinese, Japanese, Korean, Arabic, Russian, Thai and Vietnamese, plus most of Europe.
- **Vision does NOT read** Bangla, Hindi, Tamil, Urdu, or most South Asian scripts.
- **Bundled Tesseract 5.5.3** (`Contents/Resources/tesseract`) covers those. Data: `ben` (best-quality model) plus `eng hin urd tam tel mar nep guj pan mal kan sin` (fast models).

**How results are chosen**
- `recognize()` runs both engines. If Tesseract's output has at least 3 Indic letters making up 12% or more of the letters, it wins; otherwise Vision does.
- `indexText()` stores both results so search matches either.
- **Settings → OCR Languages** sets the extra languages (stored in `ocrExtraLanguages`; default `["ben"]`). Changing them re-indexes everything.

**`OCRIndexer`**
- OCRs every image without `ocrVersion == 2` and a matching language key: one at a time, in the background, at low priority.
- Covers new images as they arrive and backfills old ones, so search ("siam", "Siam" or "SIAM", case-insensitive) finds text inside any image.

**Screen OCR** (`ScreenOCR.swift`)
- **Overlays:** one borderless window per display, at screen-saver level + 1, on all Spaces.
- **Look:** your crosshair cursor (`OCRCursor`). Nothing is drawn until you drag; then the outside dims at 22% and a size label appears.
- **Cancel:** Esc (via key handler and a local monitor), right-click, or the shortcut again. A click without dragging keeps the mode open.
- **Capture:** ScreenCaptureKit (`SCScreenshotManager`, excluding this app's windows) on macOS 14+, with a CoreGraphics fallback on macOS 13.
- **Result:** the text goes to the clipboard, with a Live Activity showing the thumbnail plus "Text copied · N characters".
- **Permission:** needs Screen Recording. A rebuilt ad-hoc app may need it re-granted (see §8).

### 6.6a Clipboard tab (2026-10-19)

- **Header tabs:** icon-only since 2026-10-29: Vault (`photo.on.rectangle.angled`) and Clipboard (`list.clipboard`); a white disc slides between them (`matchedGeometryEffect`). State lives in `IslandModel.tab`.
- **Copy feedback:** copying shows `CopiedOverlay` on the card (dark veil, a white disc that springs in, a check that draws itself, "Copied") instead of text in the header. Vault tiles do the same in CALayers (`TileView.showCopied`).
- **Expand:** text cards have an expand button; the card grows over the grid (`matchedGeometryEffect`, `Motion.expand` spring 0.44/0.84) into `ExpandedClip` with the full selectable text, Copy and Close (Esc). Close shrinks it back to its slot.
- **SVG code:** a copied SVG string renders as a preview card with an "SVG" badge; expanded it shows the preview beside the code. It drags out as a real `.svg` file, and the menu has "Save SVG to Vault".
- **`ClipboardHistory.swift`:** records text and files from `ClipboardWatcher.poll`. **Copied images are not kept in the history**; they go to the vault ("Save Copied Images"). Image clips from earlier builds are purged on launch. It skips our marker and concealed, transient or auto-generated types. Duplicates move to the top. Capped at 300 items. Stored in `clipboard.json` + `clipboard/`.
- **`ClipboardGrid`:** 3-column `LazyVGrid` of `ClipCard`s. Click copies, drag works, and the context menu has Copy, Save to Vault and Delete.
- **Order:** copying a card from the history leaves it where it is (it used to jump to the top). Copying the same text again from another app still moves it to the top.
- **Reset:** the ↺ button needs two clicks within 3 s.
- **Setting:** Settings → Clipboard History, independent of "Save Copied Images".

### 6.6b Screenshots (2026-10-19)

- **⌃⌥S** (configurable, Settings → Shortcuts) reuses the screen-OCR region picker (`ScreenOCRController.pickImage`). A click without dragging captures the whole display.
- **`Capture.swift`**: `CaptureDocument` (base pixels + annotations; renders at full Retina resolution), `CaptureCenter` (stacked previews, bottom-right), `CapturePreview` (× · drag grip · Edit/Copy/Save to Vault/Copy Text/Pin), `PinWindow`.
- **`Editor.swift`**: Shotr-style editor. Bar: Copy, Save, Pin | grip | Select, Arrow, Text, Measure, Rectangle, Filled Rectangle, Pen, colour/width, Add Capture, Blur | ··· (Highlighter, Spotlight, Counter, Oval, Line, Paste Image, Copy Text, Save As). Hover hex + Tab to copy; image size in pt.
- **"Frame" icon** in the reference was interpreted as Filled Rectangle (redaction). The **"♂" icon** became the colour/thickness picker.
- **Auto-save (2026-10-29, second pass):** every new capture is also **saved to the vault straight away** (`CaptureActions.autoSave`; Live Activity "Copied & saved"). The capture remembers that item (`sourceItem`), so Copy/Save/editor edits update it in place instead of adding copies; unchanged pixels write nothing.
- **Since 2026-10-29:** every new screenshot is **copied to the clipboard automatically** (with our marker, so it isn't re-saved). The preview card hides after **Settings → Screenshot Preview** (3 s, 5 s default, 10 s, or Until Closed); hovering pauses the countdown. Any finished interaction (Copy, Save, Copy Text, Edit, Pin, or a successful drag out) closes the card.
- Captures aren't auto-saved to the vault. **Save** adds them to the vault (source `screenshot`), and **Copy** (preview, editor or pin) also saves them there. Clipboard writes carry the internal marker.
- **Full resolution (fixed 2026-10-19):**
  - The capture rect is snapped to whole device pixels. ScreenCaptureKit runs with `captureResolution = .best`, an exact pixel width/height, and the display's colour space.
  - Exports are rendered in the capture's own colour space, and PNGs record the point size (144 dpi on Retina).
  - The editor opens at **100%** in a scroll view, pixel-aligned with no smoothing. ⌘0 is 100%, ⌘9 fits, ⌘+/⌘− and pinch zoom; clicking the size label toggles 100%/fit.
  - Pins open at actual size.
- **Dev aid:** `--capture-test <dir>` captures a region and the full main display with the real path (launch via `open -n … --args`, so it needs the app's Screen Recording permission).
- **Dev aid:** `--capture-demo <image> <dir>` renders every tool plus the preview and editor windows to PNGs.

### 6.6c Folders (2026-10-29)

- **Make one:** select images → **Group** in the bulk bar (replaced "Text"), or right-click → Move to Folder → New Folder. It's named "Group N" and goes to the front of the grid. Right-click a folder to Rename…, Copy All Images, Select All Inside, or Remove Folder (Keep Images). Right-click images to move them between folders or Take Out of Folder.
- **Look (`FolderTileView`):** Finder-style, from your reference: back panel with a tab, up to three images standing out of it as papers, a front panel with the name and count. Monochrome (light silver on black); the four colours are constants at the top of `FolderTileView`, easy to switch to Finder blue.
- **Open:** click the folder. `TrayController.openStep` inserts the images one at a time (every 70 ms, 35 ms for more than 12) right after the folder; each `TileView.emerge` springs out of the folder's mouth (scale 0.28 → 1). The folder's papers lift out and its count drops ("3 of 5" … "5 out"); when empty its front flap tips open.
- **Close:** click again. Images fly back in from last to first (`TileView.fold`, 0.34 s), each removed from the grid as it lands, and the papers drop back in.
- **Drag to group (2026-10-29):** drag one or more tiles onto a folder to add them to it (the folder grows, its flap opens and the papers lift while you hover). Drag them onto another image to make a new folder of all of them (the target image shrinks with a white ring). Dropping on an image that's out of an open folder adds to that folder. How: the grid registers only for its own private type `com.imagevault.tile`, added to the dragging pasteboard in `willBeginAt` next to the real file URLs, so outside drops still reach the island and drags to other apps still deliver files. Local drags use `.move`; `validateDrop` retargets to the item under the pointer with `.on`.
- **Focus:** while a folder is open, everything outside it (ungrouped images and other folders) fades to 30%; hovering one brings it back. It un-fades as the folder closes.
- **Search** shows a flat list (folders hidden, grouped images included). Folders can't be selected or dragged; select-all picks only images.

### 6.6d Opening jitter (fixed 2026-10-29)

- **Cause:** the open reveal scaled (0.94 → 1) and blurred the whole tray, including the AppKit `NSCollectionView`. AppKit re-snaps hosted views to whole pixels every frame, so the tiles shook while the island opened.
- **Fix:** `Reveal` (scale + blur + fade) now applies only to SwiftUI parts (header, music strip); the grid itself only fades. Don't put `scaleEffect`/`blur` on `TrayRepresentable` again.

### 6.7 Now Playing

- **Metadata and commands:** a small AppleScript run through Apple's `/usr/bin/osascript`, because MediaRemote rejects direct calls from third-party apps since macOS 15.4.
- **Polling:** every 1 s, and only while the tray is open.
- **Position:** MediaRemote's elapsed time is a snapshot taken at a timestamp. The read script returns the timestamp's age, and `Track.elapsed(at:)` projects the position forward live. Loops reset correctly because every poll is published now.
- **Seek:** command **24** (`SeekToPlaybackPosition`) with options `{kMRMediaRemoteOptionPlaybackPosition: seconds}`. Verified live. The old code sent 16 with a bare number, which broke the session. After a seek, the requested position is held for 1.5 s. `NowPlaying.seek(to:)` still exists, but since 2026-10-22 nothing in the UI calls it.
- **Artwork:** browsers advertise artwork but don't send the bytes through that path. `Helpers/ivmr.c` is a tiny dylib loaded into Apple-signed `/usr/bin/perl` via DynaLoader. It calls `MRMediaRemoteGetNowPlayingInfo` and writes the artwork. Verified: it returned a 150×150 JPEG for a song playing in Comet. The player's app icon is now only a fallback.
- **UI (2026-10-22):** one 34 pt row: 24 pt artwork (click to bring the player forward), title/artist (150 pt), waveform progress, then **previous / play-pause / next** (`TransportButton`: 24 pt, hover halo, press snap; the middle one switches between play and pause).
- **Waveform:** `WaveformProgress`, matching your reference. Thin rounded bars of seeded, audio-like heights, centered. The played part is bright and the rest dim, and the bars breathe while playing. Elapsed and remaining times show at the sides. **Display only:** `allowsHitTesting(false)`, so it can't be clicked or dragged.

### 6.8 Developer aids

```bash
IMAGEVAULT_ROOT=/tmp/iv-snap "/Applications/Image Vault.app/Contents/MacOS/ImageVault" --snapshot /tmp/shots
"/Applications/Image Vault.app/Contents/MacOS/ImageVault" --ocr /path/image.png
"/Applications/Image Vault.app/Contents/MacOS/ImageVault" --nowplaying
```

---

## 7. Windows app

You rewrote it in **Rust** around 2026-10-20, replacing the earlier C#/WPF port. That port lived in the old `~/Downloads` folder and is no longer used.

- **Stack:** Rust (edition 2024), crate `imagevault` v2.2.0, built on Win32, Direct2D, DirectComposition, DirectWrite and WIC through the `windows` crate. No runtime or framework dependencies.
- **Size:** one ~3 MB `ImageVault.exe`. At rest it uses no CPU, and Windows trims its working set to under 1 MB.
- **Parity:** the same features, layout and motion as the Mac app. The island, Live Activity, drag hint and drop target, Vault and Clipboard tabs, search and sort, selection with lasso, real-file drag-out and copy all work. So do preview, Copy Text with background indexing, Now Playing, screen OCR, screenshots with previews, pins, the editor and the notification-area icon.

**Source map**

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

**Windows equivalents**
- Ctrl instead of ⌘. The shortcuts are Ctrl+Alt+V / B / T / S, and T and S can be changed.
- A notification-area icon instead of the menu bar item.
- Windows OCR (`Windows.Media.Ocr`) instead of Vision, plus Tesseract for Bangla and other scripts. That needs `winget install UB-Mannheim.TesseractOCR`; the language data sits in `dist\tesseract\tessdata`.
- System Media Transport Controls instead of MediaRemote.
- No Screen Recording permission needed.
- Data lives in `%LOCALAPPDATA%\ImageVault`, in the same `vault.json` format.
- SF Symbols are redrawn as vector paths (`icons.rs`). There's no hardware notch, so the island always draws its own.

**2026-10-22 changes** (ported from the Mac at your request)
- **Music strip:** previous / play-pause / next on the right (`transport_button` in `nowplaying.rs`, with acts `NpPrev`, `NpToggle`, `NpNext`). New vector icons: `PlayFill`, `PauseFill`, `ForwardFill`, `BackwardFill`. The waveform is display-only and no longer pushes the `NpWave` hit; the scrub code stays but is never reached.
- **Opening:** `trigger_zone()` in `island.rs`. The window's top equals the monitor's top, so window-local y = 0 is the top pixel row, and the trigger is `y ∈ [−2, 0.5)` within the notch's width (or the Live Activity's).
- **Verification:** cross-compiled on the Mac with `cargo build --release --target x86_64-pc-windows-gnu` (clean). The fresh exe was copied to `windows/dist/ImageVault.exe`. **Not yet run on Windows.**

**Open issue: dragging a screenshot preview into WhatsApp, an AI chat or a web upload field doesn't drop** (reported 2026-10-22; you asked to leave it for now).
- Ruled out:
  - The preview uses the same real-file data object as the vault tiles (`dragdrop::files_object`, a shell data object with CF_HDROP).
  - The temp PNG (`%TEMP%\ImageVault-captures\<rand>\Screenshot <date> at <time>.png`) is written correctly and isn't deleted after the drag.
- **Most likely cause:** the preview window never captures the mouse. `capture.rs::preview_msg` starts the drag only when a `WM_MOUSEMOVE` arrives inside the small preview with the button still held, more than 3 pt from the press. A quick move leaves the window first, so `DoDragDrop` never starts.
- **Proposed fix** (~5 lines, not applied): `SetCapture` on `WM_LBUTTONDOWN` in the image/grip area, `ReleaseCapture` on button up and when the drag starts.
- **To confirm on Windows:** does the thumbnail follow the pointer during the failed drag? Do vault tiles drop into WhatsApp correctly?

## 8. Platform findings and lessons

| Finding | Consequence / fix |
| --- | --- |
| **Chrome drags between tabs or into web pages carry only strings** (URL/HTML), never files | Web apps (WhatsApp, Figma, Gemini) ignored extension drags. Fixed with `dragshim.js`, which replays the drop with real `File`s. Native apps get a file via Chrome's image drag. |
| **Chrome may strip custom drag MIME types across frames** | Vault drags were also flagged by a background broadcast (`vault:drag-state`). |
| **Synthetic replays re-enter the page's own listeners** | Caused infinite recursion (stack overflow). Fixed with a `replaying` guard. |
| **Image CDNs (Unsplash `auto=format`) send AVIF/WebP** when Chrome advertises them | WhatsApp: "format not supported"; Figma rejects AVIF. Fixed: request PNG/JPEG first and normalize every save. |
| **WhatsApp's Permissions-Policy blocks the clipboard** in iframes *and* its own page | Fixed with the host-page relay and the `copy.html` helper window. Native apps aren't affected. |
| **Browsers hold one image on the clipboard**, and pages can hand the OS one file per drag | No bulk copy in the browser. The Mac and Windows apps support multi-file copy and drag. |
| **Accessory macOS apps have no menu bar**, so text fields lose ⌘A/C/V/Z | Fixed with a hidden main menu containing an Edit menu. |
| **Key equivalents reach every view in the window** | The grid swallowed ⌘A in the search field. It now acts only when it's first responder. |
| **`NSCollectionViewFlowLayout` asserts if itemSize ≤ 0** before layout | Crashed the first launch. Guarded. |
| **Cached thumbnails call back synchronously** | The tile hadn't recorded which item it showed yet, so it dropped the image (transparent tiles after search). Fixed by setting `currentID` first. |
| **MediaRemote** (macOS 15.4+) rejects third-party callers | Use `osascript` for metadata and commands, and a dylib inside `/usr/bin/perl` for block-based calls (artwork). |
| **MediaRemote elapsed time is a snapshot** | Project it forward with the timestamp's age. |
| **Seek command** | 24 plus an options dictionary, not 16 with a number. |
| **Apple Vision has no Bangla/Hindi/Tamil** | Tesseract bundled, with dylibs relinked to `@executable_path/../lib` via `dylibbundler`. |
| **Ad-hoc signing changes on every build** | Screen Recording permission was lost after every rebuild. **Fixed 2026-10-19:** `mac/setup-signing.sh` creates a self-signed "Image Vault Local Signing" certificate in its own keychain (`~/.imagevault-signing`, added to the user keychain search list because codesign only looks there). `build.sh` signs everything with it. The designated requirement is `identifier "com.imagevault.mac" and certificate leaf = H"a6ae13f6…"`, which is stable across builds. |
| **Off-screen `layer.render`** misses some SF Symbols (outline checkmarks, `square.grid.2x2`) | A capture artifact. The select-all glyph is drawn manually. |
| **Homebrew's `dotnet@8` lacks the WindowsDesktop SDK** | Use Microsoft's `dotnet-install.sh`, installed to `~/.dotnet`. |
| **WPF `CompositionTarget.Rendering` hooks force constant redraws** | Subscribe only while visible (old C# port). |
| **Mac app vs Windows app weight** (measured 2026-10-22) | Mac: 49 MB on disk, of which 46 MB is the bundled Tesseract and its language data; the binary itself is 2.4 MB. About 175 MB of memory (peak 417 MB) and ~5% CPU when measured. Causes: the SwiftUI/AppKit baseline, the thumbnail cache (up to 600 decoded images), Vision models staying loaded after indexing, and polling timers. Windows (Rust/Direct2D) stays under 1 MB at rest. Slimming could reach ~30–50 MB, but **you chose to keep the Mac app as it is.** |
| **A hover zone as tall as the menu bar** opens the island by accident | Moving toward a browser tab bar crossed it. Fixed by requiring the very top screen edge (Mac and Windows). |

---

## 9. Current state (as of 2026-10-29)

**Mac app** (v2.2.0, installed in `/Applications/Image Vault.app` from this folder):
- **Done and installed 2026-10-29:** the feedback pass in §2 (folders, one images folder, GIF/SVG, edit, screenshot card, copy overlay, expandable clipboard cards, icon tabs, select-all icon). Checked in off-screen snapshots: folder closed/opening/open/closed, SVG tile, SVG clip card, expanded card. Your real vault migrated cleanly (41 images, no sub-folders left).
- **Second pass, installed 2026-10-29:** screenshot auto-save, hover grid, folder focus fade, jitter fix, clipboard order. Hover grid and fade checked in snapshots; the jitter fix needs your eyes on the real island.
- **Not yet confirmed by you:** the motion feel of the folder animation, GIF playback with your own GIFs, the screenshot card timer, editing a vault image.
- **Signing note:** this build was signed ad-hoc (`signed: -`) because the "Image Vault Local Signing" keychain isn't in the search list right now. Screen Recording may need re-granting; running `mac/setup-signing.sh` again restores the stable identity.
- **Done and installed 2026-10-22:**
  - The music strip has previous / play-pause / next, and the waveform is display-only. Checked in an off-screen snapshot.
  - The island opens only at the top screen edge.
- **From 2026-10-19, built and installed:**
  - Screen OCR rewrite (Esc, drag, crosshair, all monitors, shortcut capture).
  - Bangla and other scripts via Tesseract, plus the OCR Languages menu.
  - Auto-OCR indexing for search.
  - Smaller tile hover icons.
  - Player position, loop, artwork and seek fixes.
  - Search fixes: thumbnails, ⌘A, equal margins.
  - `build.sh` bundling Tesseract and `libivmr.dylib`.
- **Not yet confirmed by you in daily use:**
  - Screen OCR on both monitors.
  - A Bangla OCR result.
  - Search finding text inside images.
  - The top-edge opening and the music buttons.

**Windows app** (your Rust port):
- **2026-10-22:** the same two Mac changes were ported and cross-compiled clean. `dist/ImageVault.exe` was updated, but it hasn't been run yet: there's no Windows PC available right now.
- **Open:** the screenshot preview won't drop into WhatsApp, AI chats or web upload fields. Likely cause and proposed fix are in §7; the `SetCapture` fix was applied 2026-10-29 and still needs a Windows PC to confirm.
- **2026-10-30, cross-compiled clean, not yet run on Windows:** every Mac feature from the 2026-10-29 passes (folders with the fly-out, one images folder, live GIFs, SVG badge and Copy SVG Code, Edit on every image, screenshot auto-save/auto-copy and card timer, copy overlay, expandable clipboard cards, icon tabs, select-all icon, hover grid, focus fade, clipboard order), then the storage pass (§6.3). `vault.rs` compiles unchanged on the Mac, so a harness ran it against a throwaway vault seeded with a legacy `vault.json`: 68 checks passed (import incl. folders and a duplicate id, flat-layout migration, search incl. Bangla and 1–2 character LIKE, dedupe by hash and URL, OCR save without a grid reload, folders, delete/undo/purge with thumbnails, edit, FTS integrity, a corrupt `vault.json`). At 10,000 images: import 490 ms (once), grid load 6 ms, search 0.5 ms (3+ chars) / 4.6 ms (2 chars), OCR save 0.05 ms, add 0.3 ms, delete + reload 7 ms. `ImageVault.exe` is 5.3 MB (SQLite adds ~1.9 MB). Windows-only parts (WIC thumbnail decode and disk cache writes, GIF frames, drag and drop, the UI) still need a Windows PC.

**Storage pass, 2026-10-30 (Mac, built and tested, not installed):** SQLite index, FTS search, off-main OCR saves and the thumbnail fixes (§6.3). A test harness against a throwaway vault passed 42 checks (import, search incl. Bangla, dedupe, OCR, folders, delete/undo/purge, edit, thumbnails, 10,000-image timings), and `--snapshot` rendered every state from a migrated vault. Not yet run against a real vault in daily use; the first launch will migrate it.

**Parity gaps:** none known. Windows has every Mac feature through the 2026-10-30 storage pass (built, not yet run on Windows).

**Pending documentation:** none. This file and `mac/README.md` are current as of 2026-10-30.

## 10. Bug log

| Symptom | Root cause | Fix |
| --- | --- | --- |
| Extension drop zone never fully appeared (test harness) | `requestAnimationFrame` throttled in a hidden pane | Forced reflow instead of double rAF |
| Tiles stayed dimmed after a drag | Dim was applied in rAF after `dragend` | `setTimeout` plus a `dragActive` flag |
| Drag-out to web apps did nothing | Chrome sends only strings between tabs | `dragshim.js` with real Files |
| `postMessage` origin error on load | Message sent before the iframe loaded | Post only after `load` |
| `Cannot read properties of undefined (reading 'sendMessage')` | Orphaned panel after an extension reload | Safe `send()` plus panel rebuild |
| Stack overflow in `dragshim` | Replays re-intercepted | `replaying` guard |
| "Image format not supported" in WhatsApp | AVIF/WebP stored | Accept-header preference plus normalization |
| Clipboard blocked on WhatsApp | Permissions-Policy | Host relay, then helper window |
| Mac app: no Chrome bridge manifest | Launch aborted by an itemSize assertion | Guard `side > 1` |
| Island opened on the wrong monitor | `NSScreen.main` follows focus | Default to the notch display, else `screens[0]` |
| Select-all icon invisible in snapshots | Off-screen render artifact | Custom-drawn glyph |
| OCR mode stuck (no Esc, no drag) | Nil controller, non-key panel | Rewrite (§6.6) |
| Player position wrong or stuck | Custom `==` ignored elapsed and artwork | Real equality plus projection |
| Clicking progress removed the track | Wrong seek command (16) | Command 24 plus options dictionary |
| Browser icon instead of artwork | Artwork bytes not exposed to osascript | Perl helper dylib |
| Transparent thumbnails after search | Synchronous cache callback before `currentID` was set | Set the id first |
| Opening an image preview broke the island layout (image overflowed, header gone, path tooltip over everything) | `NSImageView` reports the image's pixel size as its natural size, and the SwiftUI host grew to fit it | `FitImageView` (no natural size, aspect-fit layer), a screen-sized decode off the main thread, path tooltip only on the name. Also added ‹ › buttons and an *n / total* counter |
| ⌘A in search selected tiles or did nothing | No Edit menu; grid stole key equivalents | Edit menu plus first-responder guard |
| Island opened by accident when moving toward a browser tab bar near the notch (2026-10-22) | The opening zone was as tall as the menu bar | Open only at the very top screen edge (Mac `triggerZone`, Windows `trigger_zone`) |
| Clicking or dragging the music waveform seeked, which you didn't want (2026-10-22) | The waveform was a scrubber | Display-only `WaveformProgress`, plus previous / play-pause / next buttons |
| **Open (Windows):** a screenshot preview dragged into WhatsApp or a chat doesn't drop | Likely: no mouse capture on the preview, so the drag never starts | Proposed `SetCapture`/`ReleaseCapture` in `capture.rs`, not applied yet (§7) |

---

## 11. Building, sharing and tooling

**Mac**
```bash
cd mac && ./build.sh            # → /Applications/Image Vault.app, launches it
```
- If `./build.sh` says "permission denied" (the file lost its executable bit in the folder move), run `bash build.sh`.
- To share it: zip `Image Vault.app`. It runs on Apple Silicon only, and needs macOS 13 or newer.
- The recipient must right-click → Open (it isn't notarized).
- For Intel Macs, it would need a universal build.

**Windows**
```bat
cd windows && build.cmd         :: on Windows → dist\ImageVault.exe (+ dist\tesseract\tessdata)
```
```bash
# cross-compile on this Mac (Rust target x86_64-pc-windows-gnu + Homebrew MinGW are installed)
cd windows && cargo build --release --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/release/ImageVault.exe dist/ImageVault.exe
```
- To share it: send `dist/ImageVault.exe`. Add the `dist/tesseract` folder if they need Bangla or other extra OCR scripts, and they'll also need Tesseract installed (`winget install UB-Mannheim.TesseractOCR`).
- SmartScreen will warn (More info → Run anyway) because the file isn't code-signed.

**Tooling installed on this Mac during the project**
- Rust (`rustup`) with the `x86_64-pc-windows-gnu` target, plus Homebrew `mingw-w64`, used to cross-compile the Windows app.
- .NET 8 SDK (official) at `~/.dotnet`. It was only needed for the old C#/WPF port and can be removed.
- Homebrew `tesseract` and `dylibbundler`, used to produce `mac/Vendor/tesseract`. The app ships its own copy, so recipients don't need Homebrew.
- Backup of the original v1 extension: `~/Downloads/image-vault-extension-v1-backup.zip`.

**Removing the warnings** for shared builds would need paid certificates: an Apple Developer ID (signing plus notarization) and a Windows code-signing certificate.
