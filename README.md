# PicoBoost

PicoBoost is a lightweight, Windows-only gaming-session utility built with
Tauri 2, Rust, TypeScript, and vanilla CSS. It applies a small set of reversible
Windows changes before a game session and restores the recorded state when the
session ends.

PicoBoost combines one configurable performance session with focused tools for
memory readiness, system cleanup, hardware status, application launching, and
visual storage analysis.

## Gaming-session workflow

The central control moves through **Ready**, **Applying**, **Active**, and
**Restoring** states. PicoBoost always starts from its Performance baseline;
there is no mode selector to manage. Session Tuning organizes the available
actions by their real effect:

- **Windows performance:** High Performance power and Windows Game Mode.
- **Background overhead:** memory readiness/balance and the background replay
  buffer.
- **Session startup:** optional launch applications.

Every reversible change is checkpointed as soon as it is made. If one restore
step fails, the remaining snapshot is kept so restoration can be attempted
again. Steam is included as the default launch choice, and users can add,
order, enable, or disable their own Windows executables.

## Memory Readiness

Memory Readiness reports physical-memory availability, commit pressure, and the
largest visible applications. It deliberately does not empty the Windows file
cache or globally trim process working sets.

Users can configure up to 12 background applications for session balancing.
When one of those applications is running during activation, PicoBoost changes
its Windows memory priority to low and records the original value. Restore puts
the original priority back. Protected Windows and PicoBoost processes are
excluded.

The separate manual close tool sends normal close requests to all top-level
windows belonging to each selected application and verifies whether the process
exited. Force close is a distinct, explicitly confirmed fallback. Elevated
applications can be contacted through a narrowly scoped one-shot administrator
helper; Explorer and shell processes remain protected.

## System information

The system-status card opens a focused hardware view containing:

- CPU name, physical cores, logical processors, maximum clock, and temperature
  when a supported sensor source is available.
- Full GPU names, driver versions, VRAM, utilization, and temperature when
  supported by the installed driver tooling.
- Installed and available memory, Windows build, and active power plan.
- Click-to-copy CPU and GPU names.

Hardware telemetry is preloaded after the main window becomes interactive so
opening the modal does not stall navigation. Unsupported sensors are reported
as unavailable rather than estimated.

## System Cleanup

Cleanup is a separate, scan-first tool. Nothing is removed by Gaming Mode.
Categories are fixed in the Rust backend and the frontend can submit only the
category IDs returned by that scan.

- **Everyday:** user temporary files, crash dumps, browser resource caches, and
  the Windows Recycle Bin.
- **Developer:** pip, NuGet HTTP, and npm download caches.
- **Advanced:** NuGet global packages, graphics shader caches, and Windows
  temporary files.

Only user temporary files and crash dumps are recommended automatically.
Temporary files must be at least 24 hours old. Browser history, cookies, saved
passwords, personal documents, the registry, and Windows Update data are not
cleanup targets. Emptying the Recycle Bin is clearly marked as permanent and is
never selected automatically.

## Storage Map

Storage Map analyzes a user-selected folder and builds a complete in-memory
index. It provides:

- A squarified folder map and a separate largest-files view.
- Proportional size blocks, percentages, ranked rows, and file-type colors.
- Instant indexed folder navigation and full-index search after the scan.
- Explicit manual selection followed by a Windows Recycle Bin operation—there
  is no permanent-delete action.
- A responsive loader, scan progress, cancellation, and a collapsible window so
  the rest of PicoBoost remains usable during conventional scans.

For local NTFS volumes, PicoBoost can request one administrator approval for a
short-lived, read-only scanner. Selected subfolders use their NTFS directory
indexes; drive-root scans stream the Master File Table. The helper writes a
compact index to a randomized temporary file, exits, and never elevates the main
application. Declining approval, unsupported filesystems, and network folders
use the parallel filesystem scanner instead.

Links and reparse points are skipped. Navigation cannot leave the selected
root, the root itself is never a deletion target, and recycle requests use
opaque IDs from the active scan rather than arbitrary paths.

## Safety model

During activation PicoBoost may:

- Switch to the built-in High Performance power plan, or create one temporary
  copy when Windows has hidden it.
- Enable the current user's Windows Game Mode preference.
- Pause the current user's historical background replay preference.
- Lower memory priority only for explicitly configured background apps.
- Launch explicitly enabled applications.

Restore returns the original power plan, gaming preferences, and recorded
memory priorities. Temporary power-plan copies are removed after restoration.
Closing PicoBoost while a session is active offers to restore first; an
incomplete restore keeps PicoBoost open and retains its recovery snapshot.

PicoBoost does not stop services, close Explorer, shut down Docker or WSL,
delete caches, flush standby memory, suspend applications, change process CPU
priority, or modify game files as part of activation.

## Development

Requirements:

- Windows 10 or Windows 11
- Node.js 18 or newer
- Stable Rust toolchain with the MSVC target
- Microsoft C++ Build Tools
- WebView2 Runtime

Install dependencies and start the development build:

```powershell
npm install
npm run tauri dev
```

Run the verification commands:

```powershell
npm run build
cd src-tauri
cargo test
cargo clippy --all-targets -- -D warnings
```

Create the current-user Windows NSIS installer:

```powershell
npm run tauri build
```

The installer is written to
`src-tauri/target/release/bundle/nsis/`. Generated executables, frontend output,
dependencies, and Rust target directories are intentionally excluded from Git.

## Project structure

```text
src/                    TypeScript UI controllers and styles
src-tauri/src/lib.rs    Native Windows commands and safety validation
src-tauri/src/main.rs   Tauri and one-shot helper entry point
src-tauri/capabilities/ Tauri IPC permissions
```
