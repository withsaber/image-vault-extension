# Image Vault for Mac

A Dynamic-Island-style image vault for macOS, written in Swift with SwiftUI and AppKit. It lives in the camera notch, or draws its own notch on displays without one. Push the pointer against the top edge of the screen above it to open your image tray, and drop images onto it from anywhere. Copy or drag images back out into any app.

## The island

| State | What it looks like |
| --- | --- |
| **Resting** | A black notch. On a MacBook with a notch it blends into the hardware notch. On other displays it draws its own, with flared top corners that melt into the screen edge. |
| **Open** | Push the pointer against the very top edge of the screen, above the notch (or above a Live Activity). The notch "breathes" a little bigger for a moment, then springs open into the tray with a touch of overshoot, and the content fades in from a soft blur slightly after the shell starts moving. Just passing near the notch, for example on the way to a browser's tab bar, doesn't open it. ⌃⌥V also opens it. |
| **Leave** | About 0.35 s after the pointer leaves, the island tucks back into the notch on a tighter spring with no bounce. |
| **Live Activity** | When an image is saved (dropped, copied, or a new screenshot), the island briefly widens with a thumbnail on the left and **Saved · N in vault** on the right. |
| **Dragging** | Start dragging an image anywhere on the Mac and the island grows a **Drop here to save** hint. Drag it over the island and it opens into a large **Drop to Vault** target. On release it shrinks straight into the Live Activity. |

Clicks outside the island's shape pass straight through to the apps underneath. Opening the island and dropping onto it give a light haptic tap on Force Touch trackpads.

## The tray

- **Two tabs.** Two icons in the header switch between **Vault** (saved images) and **Clipboard** (everything you copy); a white disc slides to the one you pick.
- **Folders.** Select images and press **Group** to put them in a folder at the front of the grid. Click the folder and its images come out one by one and line up after it while the folder empties (everything else fades back so you can focus); click again and they fly back in. Right-click a folder to rename it, copy everything in it, or remove it (the images stay). You can also drag images onto a folder to add them, or onto another image to make a new folder of them. Right-click images to move them between folders.
- **GIF and SVG.** GIFs play live in the grid. SVGs show whole with an SVG badge, drag out as `.svg` files, and their **</>** hover button copies the SVG code.
- **Hover actions.** Hovering a tile splits it into four: select (top-left), copy (top-right), edit (bottom-left), and copy text — or copy SVG code — (bottom-right).
- **Edit any image.** The pencil on every tile opens the image in the editor; Save updates it in the vault (GIFs and SVGs are saved as a new copy).
- **Clipboard tab.** Every copied text and file appears in a 3-column grid, newest first. Copied images skip the history and go straight into the vault. Each card shows the source app and how long ago it was copied. Click a card to copy it again (it stays where it is; a check animation confirms it), drag it into another app, or right-click for Copy and Delete. The expand button grows a card over the grid to read long text in full; close shrinks it back. Copied SVG code shows as a rendered preview, with the code beside it when expanded. Search filters clips too. The ↺ button resets the history; click it twice to confirm. Password-manager and other concealed or transient copies are never recorded. The history is capped at 300 items and stored in `clipboard.json`. It can be turned off with Settings → Clipboard History.
- **File-manager selection.** Click to select, ⇧-click for a range, ⌘-click to toggle, drag on empty space to lasso, ⌘A selects all, and the arrow keys move.
- **Copy, then paste anywhere.** Use the copy button on a tile, or ⌘C. One image goes on the clipboard as image data (WhatsApp, Slack, Figma, Photoshop). Several go as real files (Finder, Mail, Slack, WhatsApp).
- **Drag out.** Tiles drag out as real files, so a multi-selection lands as separate files or layers.
- **Bulk bar.** Selecting two or more images shows a white pill with Copy, Group, Reveal and Delete. ⌫ deletes and ⌘Z undoes.
- **Preview.** Press Space or double-click to preview; the arrow keys step through images, ⌘C copies, ⌘T extracts text.
- **Search and sort.** Click the magnifier (or press <kbd>/</kbd>) for a search row. Filter by name, site/domain, source or the text inside an image (case-insensitive); the sort pill cycles Newest → Oldest → Name → Size. Selection, bulk actions and previews follow the filtered list.
- **Right-click menu.** Copy Image, Copy Text, Copy Path, Open, Reveal in Finder, Get Info and Delete. Get Info opens the preview with dimensions, size, date, source and the full path in the tooltip.
- **Copy Text (OCR).** On-device Vision OCR for screenshots, photos, documents and SVGs. Uses the latest Vision revision with automatic language detection, reads the original at full resolution (EXIF orientation respected), keeps reading order, column gaps (tabs) and paragraph breaks, and retries on a 2× upscale when the text is tiny. Multi-selection copies each image's text separated by a blank line (up to 10 images per action). Bangla and other scripts Vision can't read go through a bundled Tesseract (see *OCR languages*).
- **Auto-indexing.** Every image is OCR'd once in the background, one at a time at low priority: new images as they arrive, and older ones as a backfill. Search can then find a word inside any screenshot without pressing Copy Text first.
- **Now Playing.** While something is playing, open the island and a monochrome strip appears under the grid: previous / play-pause / next, title, artist, the song's artwork (the player's app icon only as a fallback) and a waveform progress bar. The waveform's played part is bright and the rest dim, and its bars breathe while playing. It's display only (clicking or dragging it does nothing); elapsed and remaining times sit at either side, and the position stays correct across loops and track changes. Click the artwork to bring the player forward. It follows whatever macOS reports, so Music, Spotify, VLC and browser video all work. Polling only runs while the tray is open.
- **Menu bar.** A template status item: left-click opens/closes the island, right-click for Open, Copy Latest Image, Recent (copy the last five), capture toggles, vault stats, Open Vault Folder and Quit.

## Settings

Click the gear in the island, or right-click the island:

- **Island Position.** Top Left, Top Center or Top Right.
- **Display.** Pick which monitor the island lives on. Displays with a notch are marked.
- **Auto-save.** Save Copied Images and Save New Screenshots. Copies of files and password-manager or transient content are never saved.
- **Music Controls.** Turn the now-playing strip off completely. It is on by default.
- **Capture Text from Screen** and **OCR Shortcut** (Change… / Reset to ⌃⌥T).
- **OCR Languages.** Extra scripts for OCR and search (Bangla is on by default). Changing them re-indexes the vault.
- **Open at Login**, **Open Vault Folder**, **Copy Latest Image**, **Quit**.

<kbd>⌃</kbd>+<kbd>⌥</kbd>+<kbd>V</kbd> opens and closes the island from anywhere. <kbd>⌃</kbd>+<kbd>⌥</kbd>+<kbd>B</kbd> copies the newest vault image to the clipboard. <kbd>⌃</kbd>+<kbd>⌥</kbd>+<kbd>T</kbd> (configurable) captures text from the screen: drag a box with the crosshair on any display and the text goes to the clipboard. Esc, right-click or the shortcut again cancels. This needs Screen Recording permission, which may need re-granting after a rebuild because the app is signed ad-hoc.

### Screenshots

<kbd>⌃</kbd>+<kbd>⌥</kbd>+<kbd>S</kbd> (configurable under Settings → Shortcuts) takes a screenshot. Drag a box on any display, or click once to capture the whole display; Esc or right-click cancels. The screenshot is copied to the clipboard and saved to the vault straight away (edits update that saved copy), and a small floating preview appears in the bottom-right corner (newer ones stack on top). It hides after 5 seconds (Settings → Screenshot Preview: 3 s, 5 s, 10 s or Until Closed; hovering pauses it), and closes as soon as you've used it (copy, save, text, edit, pin or drag):

- **×** closes it, and **• • • •** (or the thumbnail) drags the PNG into any app.
- **Edit** (or double-click) opens the editor, **Copy** puts the image on the clipboard and keeps it in the vault, **Save** adds it to the vault, **Copy Text** runs OCR, **Pin** floats it above every window.

The editor's toolbar has Copy, Save to Vault and Pin, a drag handle, then the tools: Select & Move (V), Arrow (A), Text (T), Measure (M), Rectangle (R), Filled Rectangle (F), Pen (P), colour & thickness, Add Capture (⌘A) and Blur/Erase (B). The ··· menu adds Highlighter (H), Spotlight (S), Counter (C), Oval (O), Line (L), Paste Image (⌘V), Copy Text and Save As… (⇧⌘S). Hold Shift for 45° lines and square boxes. Use Delete to remove the selection, the arrow keys to nudge it, ⌘Z / ⇧⌘Z to undo and redo, and right-click a mark to reorder it. The right side shows the colour under the pointer (Tab copies its hex) and the image size. Edits flow back to the preview and any pin. Pinned shots drag to move; scroll or pinch to resize, double-click to edit, right-click for opacity and more, and Esc closes them.

### OCR languages

Apple Vision reads about 30 languages, including English, Chinese, Japanese, Korean, Arabic, Russian, Thai, Vietnamese and most of Europe. It doesn't read Bangla, Hindi, Tamil, Urdu or most South Asian scripts, so the app bundles Tesseract 5.5.3 for those: Bangla, Hindi, Urdu, Tamil, Telugu, Marathi, Nepali, Gujarati, Punjabi, Malayalam, Kannada and Sinhala. Both engines run on each image. Tesseract's result wins when it contains a meaningful share of those scripts; otherwise Vision's does. The search index stores both, so a search matches either.

Images are stored as normal files in `~/Library/Application Support/ImageVault/images/`. AVIF, WebP, HEIC and TIFF are converted to PNG or JPEG on save, so every app accepts them.

### How Now Playing works

macOS has no public API for reading another app's now-playing item. Since macOS 15.4 the private `MediaRemote` framework also refuses direct calls from third-party apps (`Operation not permitted`), so the app runs a small AppleScript through Apple's own `/usr/bin/osascript`, which is allowed. It reads metadata and sends play, pause, next and previous. Artwork bytes don't come through that path, so a tiny helper library (`Helpers/ivmr.c`) is loaded into Apple's `/usr/bin/perl` to read them. No extra permissions are requested; if osascript or MediaRemote is unavailable the strip simply stays hidden. You can verify what the system reports with:

```bash
"/Applications/Image Vault.app/Contents/MacOS/ImageVault" --nowplaying
```

## Building

You need Swift 5.9+; the Xcode Command Line Tools are enough.

```bash
./build.sh
```

On the first run, `setup-signing.sh` creates a local self-signed certificate ("Image Vault Local Signing") in its own keychain at `~/.imagevault-signing`. Every build is signed with it, so macOS keeps Screen Recording permission across rebuilds. Without it, the build falls back to ad-hoc signing.

This builds a release binary, wraps it in `Image Vault.app`, bundles Tesseract (`Vendor/tesseract`) and the artwork helper, signs it all, installs it to `/Applications` (or `~/Applications`), and launches it.

To render every island state to PNGs off-screen, using a throwaway vault, for design checks:

```bash
IMAGEVAULT_ROOT=/tmp/iv-snap "/Applications/Image Vault.app/Contents/MacOS/ImageVault" --snapshot /tmp/iv-shots
```

To test the OCR pipeline on a file (prints the extracted text):

```bash
"/Applications/Image Vault.app/Contents/MacOS/ImageVault" --ocr /path/to/image.png
```
