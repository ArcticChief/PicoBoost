# PicoBoost

PicoBoost is a lightweight Windows gaming-mode utility built with Tauri v2,
Rust, TypeScript, and vanilla CSS. It turns the `Arctic-GamingMode` PowerShell
workflow into a safe, reversible desktop session with selectable profiles, live
progress, and a persistent restore snapshot.

## Features

- High Performance power for the active session, with exact plan restoration.
- Windows Game Mode activation with the user's previous preference restored.
- Optional replay-buffer pause while keeping manual captures available.
- Performance, Balanced, and Minimal safe-session profiles.
- Configurable application-launch step with Steam as the built-in default.
- Ordered custom `.exe` list with per-application enable controls.
- A focused session-control banner whose central action changes from
  **Activate** to **Restore**, with clear ready, applying, active, and restoring
  states.
- Clickable system snapshot with CPU topology, physical GPU telemetry, memory,
  Windows build, and active power plan; hardware names copy with one click.
- Scan-first system cleanup for fixed everyday, developer, and advanced cache
  categories, with sizes, warnings, and explicit confirmation.
- Visual Storage Map for a user-chosen folder, with proportional usage blocks,
  complete indexed search, instant folder drill-down, largest-file discovery,
  and manual Recycle Bin selection. When read-only NTFS volume access is
  available, PicoBoost uses the filesystem metadata index directly; otherwise
  it falls back automatically to a parallel directory scan. Selected folders
  use their NTFS directory index while drive-root scans stream the MFT.

## Safety and permissions

Gaming Mode does not delete files or caches, flush process memory, stop Windows
services, close applications, terminate background processes, or shut down
Docker/WSL. Selected applications launch normally without arguments or priority
changes, and already-running applications are left alone.

Memory Readiness is user-directed and separate from Gaming Mode. It first posts
normal close requests to every top-level window and verifies which selected
applications exited. A force-close fallback is shown only for revalidated,
non-protected applications that stayed open and requires a second warning;
Explorer and Windows shell processes remain excluded. If Windows blocks a
normal close because an application is elevated, PicoBoost explains the reason
and offers a separate graceful retry through a narrowly scoped, one-shot
administrator helper before exposing force close.

Cleanup is deliberately separate from Gaming Mode. It scans only hard-coded
cache categories and never accepts arbitrary directory paths. Everyday temporary
files must be at least 24 hours old; Recycle Bin and advanced categories are not
selected automatically. Browser history, cookies, saved passwords, personal
files, the registry, and Windows Update data are outside its scope.

Storage Map is also separate from Gaming Mode. Its native folder picker creates
an in-memory scan session, and recycle commands accept only opaque IDs from that
active session. Links are skipped, navigation cannot escape the selected root,
the root itself is never a deletion target, and there is no permanent-delete
fallback. Fast NTFS mode reads metadata only and never opens file contents.

Every setting changed during activation is checkpointed immediately. Restore
returns the original power plan and gaming preferences; incomplete restores keep
their snapshot for another attempt. Closing PicoBoost during an active session
requires restoration first. Administrator access is not normally required for
the per-user workflow. Fast NTFS metadata access is the one optional feature
that needs administrator approval. PicoBoost offers a clearly explained,
one-shot read-only scanner for this operation; the helper exits after returning
the index and the main application remains non-admin. Declining approval uses
the normal parallel scanner instead.

## Development

Prerequisites: Node.js 18+, the stable Rust toolchain, Windows C++ Build Tools,
and the WebView2 runtime.

```powershell
npm install
npm run tauri dev
```

Build the production app with:

```powershell
npm run tauri build
```

Build output is written under `src-tauri/target/release/`; packaged installers
are written to its `bundle/` directory.

## Technology stack

- Tauri v2 desktop shell and IPC
- Rust backend
- TypeScript, HTML, and vanilla CSS frontend
- Vite build tooling
