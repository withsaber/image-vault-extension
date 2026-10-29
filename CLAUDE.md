# Image Vault

Two native apps that must look and behave the same: `mac/` (Swift, SwiftUI + AppKit) and `windows/` (Rust, Win32 + Direct2D). History, specs and the source map are in `KNOWLEDGEBASE.md`.

## Every change lands on both platforms

A feature or fix isn't done until it works on Mac **and** Windows.

1. **Build it on one side first.** Usually the Mac, which is the reference for look and motion. Starting on Windows is fine when the change is Windows-led; then the Mac follows it.
2. **Port it to the other side right after**, in its own session or commit. Use the first implementation as the spec, and keep names, behaviour, timings and the data format the same.
3. **If a port can't be done yet** (the platform has no equivalent, or it's blocked), write it in `KNOWLEDGEBASE.md` §9 under "Parity gaps", with the reason. Never leave a gap silently.
4. **Shared rules** (data format, storage schema, shortcuts, design tokens, spring values) change on both sides in the same piece of work.

## Checking

- Mac: `cd mac && swift build -c release`. For visuals, use `--snapshot` with `IMAGEVAULT_ROOT` pointing at a throwaway vault, never the real one.
- Windows: `cd windows && cargo build --release --target x86_64-pc-windows-gnu` from the Mac. Window, drag and hotkey behaviour still needs a real Windows PC; say so when it hasn't been checked there.

## Recording

Add each change to `KNOWLEDGEBASE.md` §2 (timeline) and §9 (current state, including what's verified on each platform).
