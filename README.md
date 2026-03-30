# ⚡ Fast Copy (Rust)

> High-speed portable file copier — rewritten in Rust for native performance and a minimal footprint.

![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-blue)
![Rust](https://img.shields.io/badge/rust-2021%20edition-orange)
![GUI](https://img.shields.io/badge/GUI-egui%200.28-purple)
![Binary](https://img.shields.io/badge/binary-3.7%20MB-brightgreen)
![License](https://img.shields.io/badge/license-MIT-green)

This is the Rust rewrite of [ahsocode/Fast_Copy](https://github.com/ahsocode/Fast_Copy) (Python/PyQt5).
Same features, native binary, ~20× smaller, ~100ms startup.

---

## ✨ Features

- 🚀 **Maximum copy speed** — rayon parallel copy for many files, clonefile CoW for large files on APFS
- 🖥️ **Native GUI** — built with [egui](https://github.com/emilk/egui) (Catppuccin Mocha dark theme)
- 📂 **Browse & Select** — in-app filesystem browser with checkboxes; pick files from multiple directories before copying
- 📦 **Tiny binary** — 3.7 MB stripped release build, no runtime dependencies
- 🔁 **Skip-on-error** — bad files are logged and skipped, copy always continues
- ⏱️ **Live stats** — speed (2-second rolling window), elapsed time, ETA
- 🖱️ **Drag & drop** — drop files or folders onto the source list
- ❌ **Cancel anytime** — cancel mid-copy, partial files cleaned up automatically
- 🌙 **Dark theme** — Catppuccin Mocha palette throughout

---

## 📸 Screenshot

```
┌─────────────────────────────────────────────────────────────┐
│  Mode: [● Auto] [○ Large File] [○ Many Files]               │
├──────────────────────────┬──────────────────────────────────┤
│  Sources                 │  Destination                     │
│  ┌──────────────────┐    │  /Volumes/Backup    [Browse]     │
│  │ videos/          │    │                                  │
│  │ photos/          │    │                                  │
│  │ project.zip      │    │                                  │
│  └──────────────────┘    │                                  │
│  [+Files] [+Folder]                                         │
│  [Browse & Select…] [✕Remove]                               │
├─────────────────────────────────────────────────────────────┤
│  ████████████████░░░░  78%                                  │
│  2.31 GB/s  |  7.8 GB / 10.0 GB    Elapsed: 00:15  ETA: 00:08 │
│  /Volumes/SSD/videos/clip_042.mp4                           │
│  Files: 42 / 500                                            │
│  Copying…                                                   │
│  [  ▶  START COPY  ]   [  ✕  CANCEL  ]                     │
└─────────────────────────────────────────────────────────────┘
```

---

## 🧠 How It Works

### Copy Modes

| Mode | Strategy | Best for |
|------|----------|---------|
| **Auto** (default) | Large if single file > 100 MB, else Small | Recommended |
| **Large File** | clonefile CoW → 64 MB chunked sequential | Single large files (ISO, video) |
| **Many Files** | rayon `par_iter` — all CPU cores | Directories, source trees, photo libraries |

### Large File Strategy
1. **clonefile** (`SYS_clonefile = 517`, macOS only) — instant copy-on-write on APFS, zero bytes actually written
2. **64 MB chunked copy** — universal fallback with per-chunk progress reporting and cancel support

### Many Files Strategy
- **rayon `par_iter`** — automatically uses all available CPU cores via work-stealing
- Files ≤ 4 MB: `std::fs::copy` (delegates to OS-optimised copy: `CopyFileEx` on Windows, `copy_file_range` on Linux)
- Files > 4 MB: 1 MB buffered copy with per-chunk `AtomicU64` progress updates
- **Coordinator thread** at 20 Hz reads `AtomicU64` counters and sends progress snapshots — workers never lock a mutex for progress reporting

### Progress Reporting
- `AtomicU64` for bytes done, `AtomicBool` (`rayon_done`) to signal coordinator after `par_iter` completes
- 2-second rolling window speed tracker (samples with `Instant` + bytes)
- Monotonic progress bar — never goes backwards

---

## 📂 Browse & Select

Click **Browse & Select…** in the source panel to open the in-app filesystem browser:

- Folders listed first, then files alphabetically
- Check any combination of files and folders
- Double-click a folder to navigate into it; **↑ Up** to go back
- **Select All / Clear All** bulk actions
- Checked state persists across navigation — pick from multiple directories before clicking **Add N Selected**

---

## 📥 Download

> Grab the latest binary from the [**Releases**](../../releases) page.

| Platform | File | Notes |
|----------|------|-------|
| macOS | `Fast_Copy-macOS.zip` | Extract → double-click `Fast_Copy.app` |
| Windows | `Fast_Copy.exe` | Portable — no install needed |
| Linux | Build from source | See below |

---

## 🛠️ Build from Source

### Requirements
- Rust 1.75+ (`rustup` recommended)
- macOS: Xcode Command Line Tools
- Linux: `libgtk-3-dev`, `libxcb-*` packages (for egui/eframe)
- Windows: Visual Studio Build Tools

### Build
```bash
git clone https://github.com/ahsocode/Fast_Copy_Rust.git
cd Fast_Copy_Rust

# Debug build (fast compile, includes symbols)
cargo build

# Release build (optimised, LTO, stripped — 3.7 MB)
cargo build --release

# Run directly
cargo run --release
```

The release binary is at `target/release/fast_copy`.

### macOS — Build .app Bundle

```bash
cargo build --release

# Create bundle structure
mkdir -p Fast_Copy.app/Contents/{MacOS,Resources}
cp target/release/fast_copy Fast_Copy.app/Contents/MacOS/
cp Fast_Copy.app/Contents/Info.plist Fast_Copy.app/Contents/

# Clear extended attributes and sign (ad-hoc)
xattr -cr Fast_Copy.app
codesign --force --deep --sign - Fast_Copy.app

# Package as DMG (optional)
hdiutil create -volname "Fast Copy" -srcfolder Fast_Copy.app \
  -ov -format UDZO Fast_Copy.dmg
```

---

## 📁 Project Structure

```
fast_copy_rs/
├── Cargo.toml              # Dependencies: eframe, egui, rfd, rayon, walkdir, crossbeam-channel
├── src/
│   ├── main.rs             # Entry point — window config, eframe bootstrap
│   ├── app.rs              # GUI: FastCopyApp, BrowserState, all egui rendering
│   └── engine/
│       └── mod.rs          # Copy engine: CopyEngine, run_copy, scan_sources,
│                           #   copy_large_file, copy_small_files (rayon),
│                           #   copy_buffered, SpeedTracker
│
└── Fast_Copy.app/          # Pre-built macOS .app bundle (ad-hoc signed)
    └── Contents/
        ├── Info.plist
        ├── MacOS/fast_copy
        └── Resources/icon.icns
```

---

## ⚙️ Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `eframe` | 0.28 | App framework (window, event loop) |
| `egui` | 0.28 | Immediate-mode GUI rendering |
| `rfd` | 0.15 | Native file/folder picker dialogs |
| `rayon` | 1.10 | Data-parallel many-files copy |
| `walkdir` | 2.5 | Recursive directory traversal |
| `crossbeam-channel` | 0.5 | Lock-free progress channel (engine → GUI) |
| `libc` | 0.2 | `SYS_clonefile` syscall on macOS |

---

## 🔄 Release Profile

```toml
[profile.release]
opt-level     = 3
lto           = true
codegen-units = 1
strip         = true
```

LTO + single codegen unit gives maximum optimisation at the cost of longer compile time. Strip removes debug symbols for the smallest possible binary.

---

## 🆚 Rust vs Python — When to Use Which

| Scenario | Rust wins | Python wins |
|----------|-----------|-------------|
| Startup time | ✅ ~100ms | ❌ ~2s |
| Binary size | ✅ 3.7 MB | ❌ ~80 MB |
| RAM idle | ✅ ~20 MB | ❌ ~70 MB |
| Many small files | ✅ rayon (all cores) | ❌ max 4 threads |
| Large file, same volume | ≈ tie (both clonefile) | ≈ tie |
| Large file, cross-volume | ❌ chunked only | ✅ sendfile (zero-copy) |
| Drive-adaptive tuning | ❌ | ✅ SSD/HDD/NVMe detection |
| Windows long paths >260 chars | ❌ | ✅ `\\?\` prefix |
| Pre-flight space check | ❌ | ✅ |

---

## 🐛 Bug Fixes Applied

| Bug | Fix |
|-----|-----|
| Coordinator deadlock when source has 0 files | `rayon_done: AtomicBool` set after `par_iter`, coordinator checks it |
| Speed trends to 0 over time | Guard `if delta > 0` before adding sample to speed tracker |
| Large files show 0% until complete | `copy_buffered` calls `fetch_add` per 1 MB chunk |
| Double I/O scan (bytes + file count) | Merged into single `scan_sources()` pass |
| Dead code panic in source list | Removed unused `to_remove` variable |

---

## 📄 License

MIT © 2025 ahsocode
