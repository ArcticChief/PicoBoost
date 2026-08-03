use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::Manager;

// ============================================================================
// PicoBoost backend
// ----------------------------------------------------------------------------
// A lightweight, safety-first evolution of the `Arctic-GamingMode` workflow.
// The frontend orchestrates a deliberately small set of documented Windows
// gaming controls and checkpoints every reversible change before continuing.
//
// Design notes:
// - Activation never deletes files/caches, trims working sets, stops services,
//   force-closes applications, or changes undocumented scheduler/network settings.
// - User-selected background apps may receive a documented, reversible memory
//   priority hint so Windows favors game pages when real pressure occurs.
// - Cleanup is a separate scan-first tool restricted to fixed category IDs.
// - Power and per-user gaming preferences are snapshotted and exactly restored.
// - A legacy service-start command remains only to recover v1 restore snapshots.
// - PowerShell/powercfg keep the dependency surface as small as PicoNote's.
// ============================================================================

// CREATE_NO_WINDOW keeps every spawned powershell/powercfg call from flashing a
// console window over our frameless UI.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs a PowerShell script with no profile/console window and returns trimmed stdout.
/// PowerShell normally exits successfully even when a native command failed, so
/// individual scripts still verify `$LASTEXITCODE` for commands such as powercfg.
fn ps(script: &str) -> Result<String, String> {
    let mut cmd = Command::new("powershell");
    let wrapped = format!(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; $OutputEncoding=[Text.Encoding]::UTF8; try {{ {script} }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}"
    );
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &wrapped]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("Could not start PowerShell: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            if stdout.is_empty() {
                "PowerShell command failed".into()
            } else {
                stdout
            }
        } else {
            stderr
        });
    }
    Ok(stdout)
}

/// Builds a PowerShell array literal `@('a','b')` from a name list. This remains
/// only for restoring services paused by pre-2.0 PicoBoost sessions.
fn ps_array(names: &[String]) -> String {
    let items: Vec<String> = names
        .iter()
        .map(|n| format!("'{}'", n.replace('\'', "''")))
        .collect();
    format!("@({})", items.join(","))
}

#[derive(Serialize)]
pub struct SystemInfo {
    pub cpu: String,
    pub os: String,
    pub total_ram_mb: u64,
    pub free_ram_mb: u64,
}

#[derive(Serialize)]
pub struct RamInfo {
    pub total_mb: u64,
    pub free_mb: u64,
}

#[derive(Debug, Serialize)]
pub struct DisplayBrightnessInfo {
    pub brightness_percent: u32,
    pub supported_monitors: u32,
    pub total_monitors: u32,
}

#[derive(Debug, Serialize)]
pub struct DisplayBrightnessResult {
    pub brightness_percent: u32,
    pub updated_monitors: u32,
    pub supported_monitors: u32,
    pub total_monitors: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryProcess {
    pub pid: u32,
    pub name: String,
    pub title: String,
    pub working_set_mb: u64,
    pub private_mb: u64,
}

#[derive(Debug, Serialize)]
pub struct MemorySnapshot {
    pub total_mb: u64,
    pub available_mb: u64,
    pub used_percent: u32,
    pub commit_used_mb: u64,
    pub commit_limit_mb: u64,
    pub pressure: String,
    pub processes: Vec<MemoryProcess>,
}

#[derive(Debug, Serialize)]
pub struct MemoryCloseProcessResult {
    pub pid: u32,
    pub name: String,
    pub window_requests: u64,
    pub closed: bool,
    pub forced: bool,
    pub can_force: bool,
    pub needs_elevation: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct MemoryCloseResult {
    pub requested_processes: u64,
    pub close_requests: u64,
    pub closed_processes: u64,
    pub still_open_processes: u64,
    pub forced_processes: u64,
    pub failed_processes: u64,
    pub results: Vec<MemoryCloseProcessResult>,
    pub snapshot: MemorySnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryPriorityState {
    pub pid: u32,
    pub name: String,
    pub original_priority: u32,
}

#[derive(Debug, Serialize)]
pub struct MemoryBalanceResult {
    pub matched_apps: u64,
    pub balanced_processes: u64,
    pub skipped_processes: u64,
    pub states: Vec<MemoryPriorityState>,
    pub snapshot: MemorySnapshot,
}

#[derive(Debug, Serialize)]
pub struct MemoryRestoreResult {
    pub restored_processes: u64,
    pub skipped_processes: u64,
}

#[derive(Serialize, Deserialize)]
pub struct CpuDetails {
    pub name: String,
    pub physical_cores: u32,
    pub logical_processors: u32,
    pub load_percent: Option<u32>,
    pub current_clock_mhz: Option<u32>,
    pub max_clock_mhz: Option<u32>,
    pub temperature_c: Option<f64>,
}

#[derive(Serialize, Deserialize)]
pub struct GpuDetails {
    pub name: String,
    pub driver_version: String,
    pub vram_total_mb: Option<u64>,
    pub vram_used_mb: Option<u64>,
    pub utilization_percent: Option<u32>,
    pub temperature_c: Option<f64>,
}

#[derive(Serialize, Deserialize)]
pub struct MemoryDetails {
    pub total_mb: u64,
    pub available_mb: u64,
    pub module_count: u32,
    pub memory_type: String,
    pub configured_speed_mt_s: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct SystemDetails {
    pub cpu: CpuDetails,
    pub gpus: Vec<GpuDetails>,
    pub memory: MemoryDetails,
    pub os_name: String,
    pub os_build: String,
    pub active_power_plan: String,
    pub sensor_status: String,
}

#[derive(Serialize, Deserialize)]
pub struct PowerPlanState {
    pub original_guid: String,
    pub created_guid: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RegistryDwordState {
    pub existed: bool,
    pub value: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct WindowsGamingState {
    pub game_mode_enabled: Option<RegistryDwordState>,
    pub historical_video_enabled: Option<RegistryDwordState>,
}

#[derive(Serialize)]
pub struct CleanupCategory {
    pub id: String,
    pub name: String,
    pub description: String,
    pub group: String,
    pub bytes: u64,
    pub files: u64,
    pub default_selected: bool,
    pub caution: Option<String>,
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct CleanupRunResult {
    pub files_removed: u64,
    pub bytes_freed: u64,
    pub failed_items: u64,
}

#[derive(Debug, Serialize)]
pub struct LaunchApplicationResult {
    pub started: bool,
}

#[derive(Debug, Serialize)]
pub struct StorageItem {
    pub id: String,
    pub name: String,
    pub relative_path: String,
    pub is_directory: bool,
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct StorageScanResult {
    pub root: String,
    pub current: String,
    pub total_bytes: u64,
    pub files: u64,
    pub folders: u64,
    pub skipped: u64,
    pub duration_ms: u64,
    pub indexed_items: u64,
    pub scan_mode: String,
    pub children: Vec<StorageItem>,
    pub largest_files: Vec<StorageItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StorageFastModeSupport {
    pub available: bool,
    pub requires_elevation: bool,
    pub volume: Option<String>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct StorageRecycleResult {
    pub items_recycled: u64,
    pub bytes_recycled: u64,
    pub scan: StorageScanResult,
}

#[derive(Debug, Serialize)]
pub struct StorageScanProgress {
    pub running: bool,
    pub items_checked: u64,
    pub elapsed_ms: u64,
    pub workers: u64,
}

#[derive(Debug, Serialize)]
pub struct StorageSearchResult {
    pub query: String,
    pub total_matches: u64,
    pub indexed_items: u64,
    pub duration_ms: u64,
    pub items: Vec<StorageItem>,
}

#[derive(Default)]
struct StorageAnalyzerState {
    inner: Mutex<StorageAnalyzerInner>,
}

#[derive(Default)]
struct StorageAnalyzerInner {
    generation: u64,
    session: Option<StorageSession>,
    scan_control: Option<Arc<StorageScanControl>>,
}

struct StorageScanControl {
    cancelled: AtomicBool,
    visited: AtomicU64,
    workers: AtomicU64,
    started: Instant,
}

impl Default for StorageScanControl {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            visited: AtomicU64::new(0),
            workers: AtomicU64::new(1),
            started: Instant::now(),
        }
    }
}

struct StorageSession {
    root: PathBuf,
    current: PathBuf,
    current_node: Option<usize>,
    targets: HashMap<String, StorageTarget>,
    index: StorageIndex,
}

#[derive(Clone)]
struct StorageTarget {
    path: PathBuf,
    bytes: u64,
    node_index: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct StorageIndex {
    nodes: Vec<StorageIndexNode>,
    total_bytes: u64,
    files: u64,
    folders: u64,
    skipped: u64,
    duration_ms: u64,
    scan_mode: String,
}

#[derive(Serialize, Deserialize)]
struct StorageIndexNode {
    parent: Option<usize>,
    name: String,
    is_directory: bool,
    bytes: u64,
    files: u64,
    folders: u64,
    modified_ms: Option<u64>,
    subtree_end: usize,
}

struct CleanupSpec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    group: &'static str,
    default_selected: bool,
    caution: Option<&'static str>,
    min_age: Option<Duration>,
    paths: Vec<PathBuf>,
    recycle_bin: bool,
}

#[derive(Default)]
struct CleanupStats {
    files: u64,
    bytes: u64,
    failures: u64,
}

// ---- System stats ----------------------------------------------------------

#[cfg(windows)]
#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[cfg(windows)]
unsafe extern "system" {
    fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
}

#[cfg(windows)]
fn read_memory_status() -> Result<MemoryStatusEx, String> {
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `status` has the documented MEMORYSTATUSEX layout and its length
    // field is initialized before Windows writes to the mutable pointer.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err("Windows could not read physical memory status".into());
    }
    Ok(status)
}

#[cfg(windows)]
fn memory_megabytes() -> Result<(u64, u64), String> {
    let status = read_memory_status()?;
    const MB: u64 = 1024 * 1024;
    Ok((status.total_phys / MB, status.avail_phys / MB))
}

#[cfg(not(windows))]
fn memory_megabytes() -> Result<(u64, u64), String> {
    Err("PicoBoost supports Windows only".into())
}

/// One-shot machine profile for the header. CPU/OS use CIM once at startup;
/// RAM comes from the native Windows API.
#[tauri::command]
fn get_system_info() -> Result<SystemInfo, String> {
    let raw = ps("$os=$null; $cpuInfo=$null; \
         for($attempt=0;$attempt -lt 2;$attempt++){ try { \
           $os=Get-CimInstance Win32_OperatingSystem -ErrorAction Stop; \
           $cpuInfo=Get-CimInstance Win32_Processor -ErrorAction Stop | Select-Object -First 1; break \
         } catch { if($attempt -eq 1){ throw }; Start-Sleep -Milliseconds 150 } }; \
         $cpu=$cpuInfo.Name; \
         \"$($cpu.Trim())|$($os.Caption)\"")?;
    let parts: Vec<&str> = raw.split('|').collect();
    let (total_ram_mb, free_ram_mb) = memory_megabytes()?;
    Ok(SystemInfo {
        cpu: parts.first().unwrap_or(&"Unknown CPU").to_string(),
        os: parts.get(1).unwrap_or(&"Windows").to_string(),
        total_ram_mb,
        free_ram_mb,
    })
}

/// Native total/free RAM poll, used to animate the live memory gauge without
/// spawning PowerShell every few seconds.
#[tauri::command]
fn get_ram() -> Result<RamInfo, String> {
    let (total_mb, free_mb) = memory_megabytes()?;
    Ok(RamInfo { total_mb, free_mb })
}

#[cfg(windows)]
struct PhysicalMonitorBatches {
    batches: Vec<Vec<windows_sys::Win32::Devices::Display::PHYSICAL_MONITOR>>,
    logical_count: u32,
}

#[cfg(windows)]
impl Drop for PhysicalMonitorBatches {
    fn drop(&mut self) {
        use windows_sys::Win32::Devices::Display::DestroyPhysicalMonitors;
        for batch in &self.batches {
            if !batch.is_empty() {
                // SAFETY: Every handle came from GetPhysicalMonitorsFromHMONITOR,
                // remains owned by this batch, and is destroyed exactly once here.
                unsafe { DestroyPhysicalMonitors(batch.len() as u32, batch.as_ptr()) };
            }
        }
    }
}

#[cfg(windows)]
fn physical_monitor_batches() -> Result<PhysicalMonitorBatches, String> {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Devices::Display::{
        GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR,
    };
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    unsafe extern "system" fn collect_monitor(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: EnumDisplayMonitors receives the live Vec pointer below and
        // invokes this callback synchronously before that Vec leaves scope.
        unsafe { (&mut *(data as *mut Vec<HMONITOR>)).push(monitor) };
        1
    }

    let mut logical = Vec::<HMONITOR>::new();
    // SAFETY: The callback and data pointer remain valid for this synchronous
    // enumeration. Null HDC/clip rectangle request every desktop monitor.
    let enumerated = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            &mut logical as *mut Vec<HMONITOR> as LPARAM,
        )
    };
    if enumerated == 0 {
        return Err("Windows could not enumerate the connected displays".into());
    }

    let logical_count = logical.len() as u32;
    let mut batches = Vec::new();
    for monitor in logical {
        let mut count = 0u32;
        // SAFETY: `monitor` was produced by EnumDisplayMonitors and `count` is a
        // valid output pointer for the duration of the call.
        if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) } == 0
            || count == 0
        {
            continue;
        }
        let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
        // SAFETY: The vector has exactly `count` initialized writable entries.
        if unsafe { GetPhysicalMonitorsFromHMONITOR(monitor, count, physical.as_mut_ptr()) } != 0 {
            batches.push(physical);
        }
    }
    Ok(PhysicalMonitorBatches {
        batches,
        logical_count,
    })
}

fn brightness_percent(minimum: u32, current: u32, maximum: u32) -> u32 {
    if maximum <= minimum {
        return 0;
    }
    let bounded = current.clamp(minimum, maximum);
    (((bounded - minimum) as u64 * 100) / (maximum - minimum) as u64) as u32
}

fn brightness_value(minimum: u32, maximum: u32, percent: u32) -> u32 {
    if maximum <= minimum {
        return minimum;
    }
    minimum + (((maximum - minimum) as u64 * percent.min(100) as u64) / 100) as u32
}

#[cfg(windows)]
fn wmi_display_brightness() -> Option<(u32, u32)> {
    let raw = ps("$items=@(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness -ErrorAction SilentlyContinue); if($items.Count -gt 0){ $average=[Math]::Round(($items | Measure-Object -Property CurrentBrightness -Average).Average); Write-Output (\"$average|$($items.Count)\") }").ok()?;
    let line = raw.lines().last()?;
    let mut fields = line.split('|');
    let percent = fields.next()?.trim().parse::<u32>().ok()?.min(100);
    let count = fields.next()?.trim().parse::<u32>().ok()?;
    (count > 0).then_some((percent, count))
}

#[cfg(windows)]
fn set_wmi_display_brightness(percent: u32) -> u32 {
    let script = format!(
        "$updated=0; $items=@(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods -ErrorAction SilentlyContinue); foreach($item in $items){{ try {{ $result=Invoke-CimMethod -InputObject $item -MethodName WmiSetBrightness -Arguments @{{ Timeout=[uint32]0; Brightness=[byte]{percent} }} -ErrorAction Stop; if($result.ReturnValue -eq 0){{ $updated++ }} }} catch {{ }} }}; Write-Output $updated"
    );
    ps(&script)
        .ok()
        .and_then(|raw| raw.lines().last()?.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

#[cfg(windows)]
fn get_display_brightness_impl() -> Result<DisplayBrightnessInfo, String> {
    use windows_sys::Win32::Devices::Display::GetMonitorBrightness;

    let monitors = physical_monitor_batches()?;
    let physical_count = monitors.batches.iter().map(Vec::len).sum::<usize>() as u32;
    let total_monitors = physical_count.max(monitors.logical_count);
    let mut supported = 0u32;
    let mut percent_total = 0u64;
    for monitor in monitors.batches.iter().flatten() {
        let mut minimum = 0u32;
        let mut current = 0u32;
        let mut maximum = 0u32;
        // SAFETY: The physical monitor handle is owned by `monitors`; all
        // output pointers are valid until the call returns.
        if unsafe {
            GetMonitorBrightness(
                monitor.hPhysicalMonitor,
                &mut minimum,
                &mut current,
                &mut maximum,
            )
        } != 0
            && maximum > minimum
        {
            supported += 1;
            percent_total += brightness_percent(minimum, current, maximum) as u64;
        }
    }

    let mut total_monitors = total_monitors;
    if supported < total_monitors || total_monitors == 0 {
        if let Some((percent, count)) = wmi_display_brightness() {
            total_monitors = total_monitors.max(count);
            let additional = count.min(total_monitors.saturating_sub(supported));
            percent_total += percent as u64 * additional as u64;
            supported += additional;
        }
    }
    Ok(DisplayBrightnessInfo {
        brightness_percent: if supported > 0 {
            (percent_total / supported as u64) as u32
        } else {
            50
        },
        supported_monitors: supported,
        total_monitors,
    })
}

#[cfg(not(windows))]
fn get_display_brightness_impl() -> Result<DisplayBrightnessInfo, String> {
    Err("Display brightness control is available on Windows only".into())
}

#[cfg(windows)]
fn set_display_brightness_impl(percent: u32) -> Result<DisplayBrightnessResult, String> {
    use windows_sys::Win32::Devices::Display::{GetMonitorBrightness, SetMonitorBrightness};

    let requested = percent.min(100);
    let monitors = physical_monitor_batches()?;
    let physical_count = monitors.batches.iter().map(Vec::len).sum::<usize>() as u32;
    let mut total_monitors = physical_count.max(monitors.logical_count);
    let mut supported = 0u32;
    let mut updated = 0u32;
    for monitor in monitors.batches.iter().flatten() {
        let mut minimum = 0u32;
        let mut current = 0u32;
        let mut maximum = 0u32;
        // SAFETY: The physical monitor handle is valid and owned by `monitors`.
        if unsafe {
            GetMonitorBrightness(
                monitor.hPhysicalMonitor,
                &mut minimum,
                &mut current,
                &mut maximum,
            )
        } != 0
            && maximum > minimum
        {
            supported += 1;
            let value = brightness_value(minimum, maximum, requested);
            // SAFETY: `value` is clamped to the range reported by this handle.
            if unsafe { SetMonitorBrightness(monitor.hPhysicalMonitor, value) } != 0 {
                updated += 1;
            }
        }
    }

    if supported < total_monitors || total_monitors == 0 {
        let wmi_updated = set_wmi_display_brightness(requested);
        if wmi_updated > 0 {
            total_monitors = total_monitors.max(wmi_updated);
            let additional = wmi_updated.min(total_monitors.saturating_sub(supported));
            supported += additional;
            updated += additional;
        }
    }
    Ok(DisplayBrightnessResult {
        brightness_percent: requested,
        updated_monitors: updated,
        supported_monitors: supported,
        total_monitors,
    })
}

#[cfg(not(windows))]
fn set_display_brightness_impl(_percent: u32) -> Result<DisplayBrightnessResult, String> {
    Err("Display brightness control is available on Windows only".into())
}

/// Reads the average hardware brightness across every monitor Windows can
/// control. DDC/CI is used for desktop displays; WMI covers laptop panels.
#[tauri::command]
async fn get_display_brightness() -> Result<DisplayBrightnessInfo, String> {
    tauri::async_runtime::spawn_blocking(get_display_brightness_impl)
        .await
        .map_err(|error| format!("Brightness worker stopped: {error}"))?
}

/// Applies one normalized percentage to every controllable monitor. Unsupported
/// monitors are left untouched instead of falling back to a fake gamma overlay.
#[tauri::command]
async fn set_display_brightness(percent: u32) -> Result<DisplayBrightnessResult, String> {
    tauri::async_runtime::spawn_blocking(move || set_display_brightness_impl(percent))
        .await
        .map_err(|error| format!("Brightness worker stopped: {error}"))?
}

#[cfg(windows)]
fn read_memory_snapshot() -> Result<MemorySnapshot, String> {
    const MB: u64 = 1024 * 1024;
    let status = read_memory_status()?;
    let current_pid = std::process::id();
    let current_pid_text = current_pid.to_string();
    let script = r#"
$selfId = $PICOBOOST_PID
$protected = @('explorer','shellexperiencehost','startmenuexperiencehost','searchhost','searchapp','dwm','winlogon','csrss','services','lsass','svchost','registry','system','idle','applicationframehost','picoboost')
$rows = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
  $_.Id -ne $selfId -and $_.MainWindowHandle -ne 0 -and $protected -notcontains $_.ProcessName.ToLowerInvariant()
} | ForEach-Object {
  try {
    [pscustomobject]@{
      pid = [uint32]$_.Id
      name = [string]$_.ProcessName
      title = [string]$_.MainWindowTitle
      working_set_mb = [uint64][math]::Ceiling($_.WorkingSet64 / 1MB)
      private_mb = [uint64][math]::Ceiling($_.PrivateMemorySize64 / 1MB)
    }
  } catch {}
} | Sort-Object private_mb -Descending | Select-Object -First 16)
ConvertTo-Json -InputObject @($rows) -Compress -Depth 3
"#
    .replace("$PICOBOOST_PID", &current_pid_text);
    let raw = ps(&script)?;
    let value: serde_json::Value = if raw.trim().is_empty() {
        serde_json::Value::Array(Vec::new())
    } else {
        serde_json::from_str(&raw)
            .map_err(|error| format!("Could not read application memory list: {error}"))?
    };
    let processes = match value {
        serde_json::Value::Array(rows) => rows
            .into_iter()
            .filter_map(|row| serde_json::from_value(row).ok())
            .collect(),
        object @ serde_json::Value::Object(_) => serde_json::from_value(object)
            .map(|process| vec![process])
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let total_mb = status.total_phys / MB;
    let available_mb = status.avail_phys / MB;
    let used_percent = total_mb
        .saturating_sub(available_mb)
        .saturating_mul(100)
        .checked_div(total_mb)
        .unwrap_or(0) as u32;
    let pressure = if used_percent >= 92 && available_mb < 2_048 {
        "Critical"
    } else if used_percent >= 85 && available_mb < 4_096 {
        "Tight"
    } else {
        "Ready"
    };
    Ok(MemorySnapshot {
        total_mb,
        available_mb,
        used_percent,
        commit_used_mb: status
            .total_page_file
            .saturating_sub(status.avail_page_file)
            / MB,
        commit_limit_mb: status.total_page_file / MB,
        pressure: pressure.into(),
        processes,
    })
}

#[cfg(not(windows))]
fn read_memory_snapshot() -> Result<MemorySnapshot, String> {
    Err("Memory Readiness is available only on Windows".into())
}

#[tauri::command]
async fn get_memory_snapshot() -> Result<MemorySnapshot, String> {
    tauri::async_runtime::spawn_blocking(read_memory_snapshot)
        .await
        .map_err(|error| format!("Memory scan worker stopped: {error}"))?
}

#[cfg(windows)]
fn memory_process_image_name(pid: u32) -> Result<Option<String>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: the returned process handle is checked and always closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // A PID that vanished between refresh and click is an ordinary outcome.
        return if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            Ok(None)
        } else {
            Err("Windows denied access to verify this application".into())
        };
    }
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    // SAFETY: `buffer` is writable for `length` UTF-16 code units and `handle`
    // remains valid until CloseHandle immediately after this call.
    let queried =
        unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
    unsafe { CloseHandle(handle) };
    if queried == 0 {
        return Err("Windows could not verify the selected application".into());
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    Ok(Path::new(&path)
        .file_stem()
        .map(|name| name.to_string_lossy().to_string()))
}

#[cfg(windows)]
fn memory_process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: the handle is used only for an exit-code query and is closed.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // Access denied means we cannot prove that it exited, so report it as
        // still open instead of presenting a false success.
        return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
    }
    let mut exit_code = 0u32;
    let queried = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    unsafe { CloseHandle(handle) };
    queried == 0 || exit_code == STILL_ACTIVE as u32
}

#[cfg(windows)]
struct MemoryCloseWindowContext {
    targets: HashSet<u32>,
    requests: HashMap<u32, u64>,
    elevation_blocked: HashSet<u32>,
}

#[cfg(windows)]
unsafe extern "system" fn request_memory_window_close(
    window: windows_sys::Win32::Foundation::HWND,
    context: windows_sys::Win32::Foundation::LPARAM,
) -> i32 {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };

    let context = unsafe { &mut *(context as *mut MemoryCloseWindowContext) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if context.targets.contains(&pid) {
        if unsafe { PostMessageW(window, WM_CLOSE, 0, 0) } != 0 {
            *context.requests.entry(pid).or_default() += 1;
        } else if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
            context.elevation_blocked.insert(pid);
        }
    }
    1
}

fn encode_memory_process_name(name: &str) -> String {
    name.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_memory_process_name(encoded: &str) -> Option<String> {
    if encoded.is_empty() || !encoded.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect();
    String::from_utf8(bytes?).ok()
}

#[cfg(windows)]
fn validated_storage_helper_output(encoded: &str) -> Result<PathBuf, String> {
    let decoded = decode_memory_process_name(encoded)
        .ok_or("The elevated storage helper received an invalid output token")?;
    let path = PathBuf::from(decoded);
    let parent = path
        .parent()
        .ok_or("The elevated storage helper output has no parent folder")?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("Could not validate helper output folder: {error}"))?;
    let canonical_temp = fs::canonicalize(std::env::temp_dir())
        .map_err(|error| format!("Could not validate the Windows temporary folder: {error}"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("The elevated storage helper output name is invalid")?;
    if canonical_parent != canonical_temp
        || !file_name.starts_with("PicoBoost-Storage-")
        || !file_name.ends_with(".bin")
        || path.exists()
    {
        return Err("The elevated storage helper refused an unsafe output path".into());
    }
    Ok(path)
}

/// Handles a single read-only MFT scan with administrator rights, writes the
/// compact index to a random file in the current user's temporary folder, and
/// exits before Tauri creates a window. The main PicoBoost process never gains
/// elevation and the helper accepts no delete or mutation operation.
#[cfg(windows)]
pub fn run_storage_scan_helper_if_requested() -> Option<i32> {
    use std::io::BufWriter;

    let argument = std::env::args().find_map(|argument| {
        argument
            .strip_prefix("--picoboost-storage-helper=")
            .map(str::to_string)
    })?;
    let Some((encoded_root, encoded_output)) = argument.split_once('|') else {
        return Some(2);
    };
    let Some(root_text) = decode_memory_process_name(encoded_root) else {
        return Some(2);
    };
    let output = match validated_storage_helper_output(encoded_output) {
        Ok(output) => output,
        Err(_) => return Some(2),
    };
    let root = match fs::canonicalize(root_text) {
        Ok(root) if is_safe_directory_root(&root) => root,
        _ => return Some(2),
    };
    let file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
    {
        Ok(file) => file,
        Err(_) => return Some(3),
    };
    let control = StorageScanControl::default();
    let result = build_preferred_ntfs_storage_index(&root, &control);
    let written = bincode::serialize_into(BufWriter::new(file), &result).is_ok();
    Some(if written { 0 } else { 3 })
}

#[cfg(not(windows))]
pub fn run_storage_scan_helper_if_requested() -> Option<i32> {
    None
}

#[cfg(windows)]
fn build_elevated_ntfs_storage_index(root: &Path) -> Result<StorageIndex, String> {
    use std::io::BufReader;

    let executable =
        std::env::current_exe().map_err(|error| format!("Could not locate PicoBoost: {error}"))?;
    let output = std::env::temp_dir().join(format!(
        "PicoBoost-Storage-{}.bin",
        uuid::Uuid::new_v4().simple()
    ));
    let argument = format!(
        "--picoboost-storage-helper={}|{}",
        encode_memory_process_name(&display_storage_path(root)),
        encode_memory_process_name(&output.to_string_lossy())
    );
    let executable = executable.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$process=Start-Process -FilePath '{executable}' -Verb RunAs -ArgumentList '{argument}' -PassThru -Wait; \
         if($null -eq $process){{throw 'Administrator scanner did not start'}}; $process.ExitCode"
    );
    let helper_result = (|| {
        let exit_code = ps(&script)?
            .trim()
            .parse::<i32>()
            .map_err(|_| "Administrator scanner returned an invalid result".to_string())?;
        if exit_code != 0 {
            return Err("Administrator scanner could not read the selected NTFS index".into());
        }
        let file = File::open(&output)
            .map_err(|error| format!("Could not open the completed storage index: {error}"))?;
        bincode::deserialize_from::<_, Result<StorageIndex, String>>(BufReader::new(file))
            .map_err(|error| format!("Could not decode the completed storage index: {error}"))?
    })();
    let _ = fs::remove_file(output);
    helper_result
}

#[cfg(not(windows))]
fn build_elevated_ntfs_storage_index(_root: &Path) -> Result<StorageIndex, String> {
    Err("Fast MFT scanning is available on Windows only".into())
}

#[cfg(windows)]
fn run_elevated_memory_close_helper(
    processes: &[MemoryProcess],
    force: bool,
) -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("Could not locate PicoBoost: {error}"))?;
    let targets = processes
        .iter()
        .map(|process| {
            format!(
                "{}:{}",
                process.pid,
                encode_memory_process_name(process.name.trim().trim_end_matches(".exe"))
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let mode = if force { "force" } else { "graceful" };
    let argument = format!("--picoboost-close-helper={mode}|{targets}");
    let executable = executable.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$process=Start-Process -FilePath '{executable}' -Verb RunAs -ArgumentList '{argument}' -PassThru -Wait; \
         if($null -eq $process){{throw 'Administrator helper did not start'}}; $process.ExitCode"
    );
    let exit_code = ps(&script)?
        .trim()
        .parse::<i32>()
        .map_err(|_| "Administrator helper returned an invalid result".to_string())?;
    if exit_code == 0 {
        Ok(())
    } else {
        Err("Administrator helper could not validate the selected application".into())
    }
}

/// Handles the narrowly scoped elevated close helper before Tauri creates a
/// window. Every PID is paired with its executable name and revalidated; shell
/// and PicoBoost processes remain protected even if this argument is forged.
#[cfg(windows)]
pub fn run_memory_close_helper_if_requested() -> Option<i32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

    let argument = std::env::args().find_map(|argument| {
        argument
            .strip_prefix("--picoboost-close-helper=")
            .map(str::to_string)
    })?;
    let (mode, targets) = argument.split_once('|')?;
    if !matches!(mode, "graceful" | "force") {
        return Some(2);
    }
    let mut verified = Vec::new();
    for target in targets.split(',').filter(|target| !target.is_empty()) {
        let Some((pid, encoded_name)) = target.split_once(':') else {
            return Some(2);
        };
        let Ok(pid) = pid.parse::<u32>() else {
            return Some(2);
        };
        let Some(expected) = decode_memory_process_name(encoded_name) else {
            return Some(2);
        };
        if pid == std::process::id() || protected_memory_process(&expected) {
            continue;
        }
        if memory_process_image_name(pid)
            .ok()
            .flatten()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(&expected))
        {
            verified.push(pid);
        }
    }
    if mode == "force" {
        for pid in verified {
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                    0,
                    pid,
                )
            };
            if !handle.is_null() {
                unsafe {
                    TerminateProcess(handle, 1);
                    CloseHandle(handle);
                }
            }
        }
    } else {
        let mut context = MemoryCloseWindowContext {
            targets: verified.iter().copied().collect(),
            requests: HashMap::new(),
            elevation_blocked: HashSet::new(),
        };
        unsafe {
            EnumWindows(
                Some(request_memory_window_close),
                (&mut context as *mut MemoryCloseWindowContext) as isize,
            )
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && verified.iter().any(|pid| memory_process_is_running(*pid))
        {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Some(0)
}

#[cfg(not(windows))]
pub fn run_memory_close_helper_if_requested() -> Option<i32> {
    None
}

#[cfg(windows)]
fn validate_memory_close_targets(
    processes: Vec<MemoryProcess>,
) -> Result<(Vec<MemoryProcess>, Vec<MemoryCloseProcessResult>), String> {
    if processes.is_empty() || processes.len() > 10 {
        return Err("Select between 1 and 10 visible applications".into());
    }
    let current_pid = std::process::id();
    let mut seen = HashSet::new();
    let mut valid = Vec::new();
    let mut results = Vec::new();
    for process in processes {
        if !seen.insert(process.pid) {
            continue;
        }
        let expected = process.name.trim().trim_end_matches(".exe");
        if process.pid == current_pid || protected_memory_process(expected) {
            results.push(MemoryCloseProcessResult {
                pid: process.pid,
                name: process.name,
                window_requests: 0,
                closed: false,
                forced: false,
                can_force: false,
                needs_elevation: false,
                detail: "Protected application was not touched".into(),
            });
            continue;
        }
        match memory_process_image_name(process.pid) {
            Ok(None) => results.push(MemoryCloseProcessResult {
                pid: process.pid,
                name: process.name,
                window_requests: 0,
                closed: true,
                forced: false,
                can_force: false,
                needs_elevation: false,
                detail: "Application had already closed".into(),
            }),
            Ok(Some(actual)) if actual.eq_ignore_ascii_case(expected) => valid.push(process),
            Ok(Some(_)) => results.push(MemoryCloseProcessResult {
                pid: process.pid,
                name: process.name,
                window_requests: 0,
                closed: false,
                forced: false,
                can_force: false,
                needs_elevation: false,
                detail: "PID identity changed; request refused".into(),
            }),
            Err(detail) => results.push(MemoryCloseProcessResult {
                pid: process.pid,
                name: process.name,
                window_requests: 0,
                closed: false,
                forced: false,
                can_force: false,
                needs_elevation: false,
                detail,
            }),
        }
    }
    Ok((valid, results))
}

#[cfg(windows)]
fn close_visible_memory_apps(processes: Vec<MemoryProcess>) -> Result<MemoryCloseResult, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

    let requested_processes = processes.len() as u64;
    let (valid, mut results) = validate_memory_close_targets(processes)?;
    let mut context = MemoryCloseWindowContext {
        targets: valid.iter().map(|process| process.pid).collect(),
        requests: HashMap::new(),
        elevation_blocked: HashSet::new(),
    };
    if !context.targets.is_empty() {
        // SAFETY: EnumWindows invokes the callback synchronously; `context`
        // therefore remains alive and exclusively borrowed for the full call.
        unsafe {
            EnumWindows(
                Some(request_memory_window_close),
                (&mut context as *mut MemoryCloseWindowContext) as isize,
            )
        };
    }

    // Give normal shutdown, save prompts, and multi-process application cleanup
    // a bounded amount of time. Return early as soon as every selected PID exits.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && valid
            .iter()
            .any(|process| memory_process_is_running(process.pid))
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    for process in valid {
        let requests = context.requests.get(&process.pid).copied().unwrap_or(0);
        let closed = !memory_process_is_running(process.pid);
        results.push(MemoryCloseProcessResult {
            pid: process.pid,
            name: process.name,
            window_requests: requests,
            closed,
            forced: false,
            can_force: !closed,
            needs_elevation: !closed && context.elevation_blocked.contains(&process.pid),
            detail: if closed {
                "Application exited normally".into()
            } else if context.elevation_blocked.contains(&process.pid) {
                "Windows requires administrator approval to contact this application".into()
            } else if requests == 0 {
                "No closable window accepted the request".into()
            } else {
                "Application remained open or is waiting for user input".into()
            },
        });
    }
    let close_requests = results.iter().map(|result| result.window_requests).sum();
    let closed_processes = results.iter().filter(|result| result.closed).count() as u64;
    let still_open_processes = requested_processes.saturating_sub(closed_processes);
    Ok(MemoryCloseResult {
        requested_processes,
        close_requests,
        closed_processes,
        still_open_processes,
        forced_processes: 0,
        failed_processes: results
            .iter()
            .filter(|result| !result.closed && result.window_requests == 0)
            .count() as u64,
        results,
        snapshot: read_memory_snapshot()?,
    })
}

#[cfg(not(windows))]
fn close_visible_memory_apps(_processes: Vec<MemoryProcess>) -> Result<MemoryCloseResult, String> {
    Err("Memory Readiness is available only on Windows".into())
}

#[tauri::command]
async fn close_memory_apps(processes: Vec<MemoryProcess>) -> Result<MemoryCloseResult, String> {
    tauri::async_runtime::spawn_blocking(move || close_visible_memory_apps(processes))
        .await
        .map_err(|error| format!("Memory recovery worker stopped: {error}"))?
}

#[cfg(windows)]
fn close_visible_memory_apps_elevated(
    processes: Vec<MemoryProcess>,
    force: bool,
) -> Result<MemoryCloseResult, String> {
    let requested_processes = processes.len() as u64;
    let (valid, mut results) = validate_memory_close_targets(processes)?;
    if !valid.is_empty() {
        run_elevated_memory_close_helper(&valid, force)?;
    }
    for process in valid {
        let closed = !memory_process_is_running(process.pid);
        results.push(MemoryCloseProcessResult {
            pid: process.pid,
            name: process.name,
            window_requests: u64::from(!force),
            closed,
            forced: force && closed,
            can_force: !closed,
            needs_elevation: false,
            detail: if closed && force {
                "Application was force closed with administrator approval".into()
            } else if closed {
                "Application exited normally after administrator approval".into()
            } else if force {
                "Application remained active after the elevated force-close request".into()
            } else {
                "Application remained open after the elevated normal-close request".into()
            },
        });
    }
    let closed_processes = results.iter().filter(|result| result.closed).count() as u64;
    let forced_processes = results.iter().filter(|result| result.forced).count() as u64;
    Ok(MemoryCloseResult {
        requested_processes,
        close_requests: results.iter().map(|result| result.window_requests).sum(),
        closed_processes,
        still_open_processes: requested_processes.saturating_sub(closed_processes),
        forced_processes,
        failed_processes: results.iter().filter(|result| !result.closed).count() as u64,
        results,
        snapshot: read_memory_snapshot()?,
    })
}

#[cfg(not(windows))]
fn close_visible_memory_apps_elevated(
    _processes: Vec<MemoryProcess>,
    _force: bool,
) -> Result<MemoryCloseResult, String> {
    Err("Memory Readiness is available only on Windows".into())
}

#[tauri::command]
async fn close_memory_apps_elevated(
    processes: Vec<MemoryProcess>,
    force: bool,
) -> Result<MemoryCloseResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        close_visible_memory_apps_elevated(processes, force)
    })
    .await
    .map_err(|error| format!("Elevated memory-close worker stopped: {error}"))?
}

#[cfg(windows)]
fn force_close_visible_memory_apps(
    processes: Vec<MemoryProcess>,
) -> Result<MemoryCloseResult, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };

    let requested_processes = processes.len() as u64;
    let (valid, mut results) = validate_memory_close_targets(processes)?;
    for process in valid {
        // Requiring both query and terminate rights lets us retain the identity
        // validation above and refuse higher-integrity/protected applications.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                0,
                process.pid,
            )
        };
        if handle.is_null() {
            results.push(MemoryCloseProcessResult {
                pid: process.pid,
                name: process.name,
                window_requests: 0,
                closed: false,
                forced: false,
                can_force: false,
                needs_elevation: true,
                detail: "Windows denied permission to end this application".into(),
            });
            continue;
        }
        // SAFETY: the process handle is scoped to this selected, revalidated PID
        // and is closed immediately after the termination request.
        let requested = unsafe { TerminateProcess(handle, 1) } != 0;
        unsafe { CloseHandle(handle) };
        if requested {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline && memory_process_is_running(process.pid) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let closed = !memory_process_is_running(process.pid);
        results.push(MemoryCloseProcessResult {
            pid: process.pid,
            name: process.name,
            window_requests: 0,
            closed,
            forced: requested && closed,
            can_force: false,
            needs_elevation: false,
            detail: if requested && closed {
                "Application was force closed".into()
            } else if requested {
                "Windows accepted the request, but the PID is still active".into()
            } else {
                "Windows refused the force-close request".into()
            },
        });
    }
    let closed_processes = results.iter().filter(|result| result.closed).count() as u64;
    let forced_processes = results.iter().filter(|result| result.forced).count() as u64;
    Ok(MemoryCloseResult {
        requested_processes,
        close_requests: 0,
        closed_processes,
        still_open_processes: requested_processes.saturating_sub(closed_processes),
        forced_processes,
        failed_processes: results.iter().filter(|result| !result.closed).count() as u64,
        results,
        snapshot: read_memory_snapshot()?,
    })
}

#[cfg(not(windows))]
fn force_close_visible_memory_apps(
    _processes: Vec<MemoryProcess>,
) -> Result<MemoryCloseResult, String> {
    Err("Memory Readiness is available only on Windows".into())
}

#[tauri::command]
async fn force_close_memory_apps(
    processes: Vec<MemoryProcess>,
) -> Result<MemoryCloseResult, String> {
    tauri::async_runtime::spawn_blocking(move || force_close_visible_memory_apps(processes))
        .await
        .map_err(|error| format!("Memory force-close worker stopped: {error}"))?
}

fn protected_memory_process(name: &str) -> bool {
    matches!(
        name.trim()
            .trim_end_matches(".exe")
            .to_ascii_lowercase()
            .as_str(),
        "explorer"
            | "shellexperiencehost"
            | "startmenuexperiencehost"
            | "searchhost"
            | "searchapp"
            | "dwm"
            | "winlogon"
            | "csrss"
            | "services"
            | "lsass"
            | "svchost"
            | "registry"
            | "system"
            | "idle"
            | "applicationframehost"
            | "picoboost"
    )
}

fn validated_memory_app_names(names: Vec<String>) -> Result<Vec<String>, String> {
    if names.len() > 12 {
        return Err("Choose no more than 12 background applications".into());
    }
    let mut clean = Vec::new();
    for name in names {
        let normalized = name.trim().trim_end_matches(".exe").to_ascii_lowercase();
        if normalized.is_empty()
            || normalized.len() > 128
            || !normalized.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
            })
            || protected_memory_process(&normalized)
        {
            return Err(format!(
                "Application name cannot be memory-balanced: {name}"
            ));
        }
        if !clean.contains(&normalized) {
            clean.push(normalized);
        }
    }
    Ok(clean)
}

#[cfg(windows)]
fn set_process_memory_priority(pid: u32, priority: u32) -> Result<u32, String> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetProcessInformation, OpenProcess, ProcessMemoryPriority, SetProcessInformation,
        MEMORY_PRIORITY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    };

    if !(1..=5).contains(&priority) {
        return Err("Memory priority is outside the Windows-supported range".into());
    }
    // SAFETY: The PID is validated against a live, non-system process list and
    // the returned handle is checked before it is used or closed.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(format!(
            "Windows denied memory-priority access to PID {pid}: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut original = MEMORY_PRIORITY_INFORMATION { MemoryPriority: 0 };
    // SAFETY: `original` is a correctly sized writable structure and `handle`
    // remains valid until the explicit CloseHandle below.
    let read_ok = unsafe {
        GetProcessInformation(
            handle,
            ProcessMemoryPriority,
            (&raw mut original).cast(),
            size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        )
    };
    let desired = MEMORY_PRIORITY_INFORMATION {
        MemoryPriority: priority,
    };
    // SAFETY: `desired` is a correctly sized immutable structure and the
    // process handle includes PROCESS_SET_INFORMATION access.
    let write_ok = if read_ok != 0 {
        unsafe {
            SetProcessInformation(
                handle,
                ProcessMemoryPriority,
                (&raw const desired).cast(),
                size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            )
        }
    } else {
        0
    };
    let error = std::io::Error::last_os_error();
    // SAFETY: `handle` was returned by OpenProcess and is closed exactly once.
    unsafe { CloseHandle(handle) };
    if read_ok == 0 || write_ok == 0 {
        return Err(format!(
            "Windows could not change PID {pid} memory priority: {error}"
        ));
    }
    Ok(original.MemoryPriority)
}

#[derive(Deserialize)]
struct MemoryProcessIdentity {
    pid: u32,
    name: String,
}

#[cfg(windows)]
fn memory_process_instances(names: &[String]) -> Result<Vec<MemoryProcessIdentity>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let names = ps_array(names);
    let current_pid = std::process::id();
    let script = format!(
        "$names={names}; $selfId={current_pid}; $rows=@(Get-Process -ErrorAction SilentlyContinue | Where-Object {{ \
           $_.Id -ne $selfId -and $names -contains $_.ProcessName.ToLowerInvariant() \
         }} | Select-Object -First 96 | ForEach-Object {{ [pscustomobject]@{{ pid=[uint32]$_.Id; name=[string]$_.ProcessName }} }}); \
         ConvertTo-Json -InputObject @($rows) -Compress -Depth 2"
    );
    let raw = ps(&script)?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw)
        .map_err(|error| format!("Could not read background application instances: {error}"))
}

#[cfg(windows)]
fn balance_memory_apps(names: Vec<String>) -> Result<MemoryBalanceResult, String> {
    const MEMORY_PRIORITY_LOW: u32 = 2;
    let names = validated_memory_app_names(names)?;
    let instances = memory_process_instances(&names)?;
    let matched_apps = instances
        .iter()
        .map(|process| process.name.to_ascii_lowercase())
        .collect::<HashSet<_>>()
        .len() as u64;
    let mut states = Vec::new();
    let mut skipped_processes = 0;
    for process in instances {
        if protected_memory_process(&process.name) {
            skipped_processes += 1;
            continue;
        }
        match set_process_memory_priority(process.pid, MEMORY_PRIORITY_LOW) {
            Ok(original_priority) => states.push(MemoryPriorityState {
                pid: process.pid,
                name: process.name,
                original_priority,
            }),
            Err(_) => skipped_processes += 1,
        }
    }
    Ok(MemoryBalanceResult {
        matched_apps,
        balanced_processes: states.len() as u64,
        skipped_processes,
        states,
        snapshot: read_memory_snapshot()?,
    })
}

#[cfg(not(windows))]
fn balance_memory_apps(_names: Vec<String>) -> Result<MemoryBalanceResult, String> {
    Err("Session memory balancing is available only on Windows".into())
}

#[tauri::command]
async fn apply_memory_balance(names: Vec<String>) -> Result<MemoryBalanceResult, String> {
    tauri::async_runtime::spawn_blocking(move || balance_memory_apps(names))
        .await
        .map_err(|error| format!("Memory balance worker stopped: {error}"))?
}

#[cfg(windows)]
fn restore_memory_apps(states: Vec<MemoryPriorityState>) -> Result<MemoryRestoreResult, String> {
    if states.len() > 96 {
        return Err("Memory restore snapshot is unexpectedly large".into());
    }
    let requested_names: HashSet<String> = states
        .iter()
        .map(|state| {
            state
                .name
                .trim()
                .trim_end_matches(".exe")
                .to_ascii_lowercase()
        })
        .collect();
    let names = validated_memory_app_names(requested_names.into_iter().collect())?;
    let live = memory_process_instances(&names)?;
    let live: HashMap<u32, String> = live
        .into_iter()
        .map(|process| (process.pid, process.name.to_ascii_lowercase()))
        .collect();
    let mut restored_processes = 0;
    let mut skipped_processes = 0;
    for state in states {
        let same_process = live.get(&state.pid).is_some_and(|name| {
            name.eq_ignore_ascii_case(state.name.trim().trim_end_matches(".exe"))
        });
        if !same_process || !(1..=5).contains(&state.original_priority) {
            skipped_processes += 1;
            continue;
        }
        if set_process_memory_priority(state.pid, state.original_priority).is_ok() {
            restored_processes += 1;
        } else {
            skipped_processes += 1;
        }
    }
    Ok(MemoryRestoreResult {
        restored_processes,
        skipped_processes,
    })
}

#[cfg(not(windows))]
fn restore_memory_apps(_states: Vec<MemoryPriorityState>) -> Result<MemoryRestoreResult, String> {
    Err("Session memory balancing is available only on Windows".into())
}

#[tauri::command]
async fn restore_memory_balance(
    states: Vec<MemoryPriorityState>,
) -> Result<MemoryRestoreResult, String> {
    tauri::async_runtime::spawn_blocking(move || restore_memory_apps(states))
        .await
        .map_err(|error| format!("Memory restore worker stopped: {error}"))?
}

/// Focused hardware information for the system-details modal. Windows does not
/// provide trustworthy CPU/GPU temperatures through a universal API, so sensor
/// values are best-effort: NVIDIA telemetry is queried directly, while CPU and
/// other GPU temperatures use a running LibreHardwareMonitor/OpenHardwareMonitor
/// WMI provider when one is available. Missing readings stay `None` rather than
/// presenting an ACPI thermal-zone value as a CPU temperature.
fn read_system_details() -> Result<SystemDetails, String> {
    let (total_mb, available_mb) = memory_megabytes()?;
    let script = r#"
$cpu = $null
$os = $null
for ($attempt = 0; $attempt -lt 2; $attempt++) {
  try {
    $cpu = Get-CimInstance Win32_Processor -ErrorAction Stop | Select-Object -First 1
    $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop | Select-Object -First 1
    break
  } catch {
    if ($attempt -eq 1) { throw }
    Start-Sleep -Milliseconds 150
  }
}
$memoryModules = @(Get-CimInstance Win32_PhysicalMemory -ErrorAction SilentlyContinue)
$gpuDevices = @(Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue | Where-Object {
  $_.Name -and ($_.PNPDeviceID -match '^PCI\\' -or $_.Name -match '(?i)nvidia|radeon|amd.+graphics|intel.+(graphics|arc)')
})
if (-not $cpu) { throw 'Windows did not return CPU information' }

$cpuPerformance = $null
try {
  $cpuPerformance = Get-CimInstance Win32_PerfFormattedData_Counters_ProcessorInformation -ErrorAction Stop |
    Where-Object { $_.Name -eq '_Total' } |
    Select-Object -First 1
} catch {}
$cpuLoad = if ($cpuPerformance -and $null -ne $cpuPerformance.PercentProcessorUtility) {
  [uint32][math]::Min(100, [math]::Round([double]$cpuPerformance.PercentProcessorUtility))
} elseif ($null -ne $cpu.LoadPercentage) {
  [uint32][math]::Min(100, [math]::Round([double]$cpu.LoadPercentage))
} else { $null }
$currentClock = if ($cpuPerformance -and $cpuPerformance.ProcessorFrequency) {
  [uint32]$cpuPerformance.ProcessorFrequency
} elseif ($cpu.CurrentClockSpeed) {
  [uint32]$cpu.CurrentClockSpeed
} else { $null }

$sensors = @()
$hardware = @()
$sensorProvider = $null
$providers = @(
  @{ Namespace = 'root/LibreHardwareMonitor'; Name = 'LibreHardwareMonitor' },
  @{ Namespace = 'root/OpenHardwareMonitor'; Name = 'OpenHardwareMonitor' }
)
foreach ($provider in $providers) {
  try {
    $candidateSensors = @(Get-CimInstance -Namespace $provider.Namespace -ClassName Sensor -ErrorAction Stop)
    if ($candidateSensors.Count -gt 0) {
      $sensors = $candidateSensors
      $hardware = @(Get-CimInstance -Namespace $provider.Namespace -ClassName Hardware -ErrorAction SilentlyContinue)
      $sensorProvider = $provider.Name
      break
    }
  } catch {}
}

$cpuHardware = $hardware | Where-Object { $_.HardwareType -match '(?i)cpu' } | Select-Object -First 1
$cpuTemperatureSensors = if ($cpuHardware) {
  @($sensors | Where-Object { $_.SensorType -eq 'Temperature' -and $_.Parent -eq $cpuHardware.Identifier })
} else {
  @($sensors | Where-Object { $_.SensorType -eq 'Temperature' -and ("$($_.Identifier) $($_.Parent) $($_.Name)" -match '(?i)cpu') })
}
$cpuTemperature = $null
if ($cpuTemperatureSensors.Count -gt 0) {
  $reading = $cpuTemperatureSensors | Where-Object { $null -ne $_.Value } | Sort-Object Value -Descending | Select-Object -First 1
  if ($reading) { $cpuTemperature = [math]::Round([double]$reading.Value, 1) }
}

$nvidiaRows = @()
$nvidiaCommand = Get-Command 'nvidia-smi.exe' -ErrorAction SilentlyContinue
if ($nvidiaCommand) {
  $lines = @(& $nvidiaCommand.Source --query-gpu=name,temperature.gpu,utilization.gpu,memory.total,memory.used,driver_version --format=csv,noheader,nounits 2>$null)
  if ($LASTEXITCODE -eq 0) {
    foreach ($line in $lines) {
      $parts = @($line -split ',' | ForEach-Object { $_.Trim() })
      if ($parts.Count -ge 6) {
        $nvidiaTemperature = $null
        $nvidiaUtilization = $null
        $nvidiaVramTotal = $null
        $nvidiaVramUsed = $null
        try { $nvidiaTemperature = [double]$parts[1] } catch {}
        try { $nvidiaUtilization = [uint32]$parts[2] } catch {}
        try { $nvidiaVramTotal = [uint64][double]$parts[3] } catch {}
        try { $nvidiaVramUsed = [uint64][double]$parts[4] } catch {}
        $nvidiaRows += [pscustomobject]@{
          name = $parts[0]
          temperature_c = $nvidiaTemperature
          utilization_percent = $nvidiaUtilization
          vram_total_mb = $nvidiaVramTotal
          vram_used_mb = $nvidiaVramUsed
          driver_version = $parts[5]
        }
      }
    }
  }
}

$gpuHardware = @($hardware | Where-Object { $_.HardwareType -match '(?i)gpu' })
$nvidiaIndex = 0
$hardwareIndex = 0
$gpuDetails = @()
foreach ($gpu in $gpuDevices) {
  $temperature = $null
  $utilization = $null
  $vramTotal = $null
  $vramUsed = $null
  $driver = if ($gpu.DriverVersion) { [string]$gpu.DriverVersion } else { 'Not reported' }

  if ($gpu.Name -match '(?i)nvidia' -and $nvidiaIndex -lt $nvidiaRows.Count) {
    $telemetry = $nvidiaRows[$nvidiaIndex]
    $nvidiaIndex++
    $temperature = $telemetry.temperature_c
    $utilization = $telemetry.utilization_percent
    $vramTotal = $telemetry.vram_total_mb
    $vramUsed = $telemetry.vram_used_mb
    if ($telemetry.driver_version) { $driver = $telemetry.driver_version }
  } else {
    $gpuSensorHardware = if ($hardwareIndex -lt $gpuHardware.Count) { $gpuHardware[$hardwareIndex] } else { $null }
    if ($gpuSensorHardware) {
      $gpuTemperatureSensors = @($sensors | Where-Object { $_.SensorType -eq 'Temperature' -and $_.Parent -eq $gpuSensorHardware.Identifier })
      $reading = $gpuTemperatureSensors | Where-Object { $null -ne $_.Value } | Sort-Object Value -Descending | Select-Object -First 1
      if ($reading) { $temperature = [math]::Round([double]$reading.Value, 1) }
    }
    # Win32_VideoController uses a 32-bit VRAM field; values near its 4 GB
    # ceiling are intentionally omitted because they are commonly truncated.
    if ($gpu.AdapterRAM -and [uint64]$gpu.AdapterRAM -lt 4200000000) {
      $vramTotal = [uint64][math]::Round([double]$gpu.AdapterRAM / 1MB)
    }
  }
  $hardwareIndex++

  $gpuDetails += [pscustomobject]@{
    name = [string]$gpu.Name
    driver_version = $driver
    vram_total_mb = $vramTotal
    vram_used_mb = $vramUsed
    utilization_percent = $utilization
    temperature_c = $temperature
  }
}

$moduleSpeed = $memoryModules | Where-Object { $_.ConfiguredClockSpeed } | Select-Object -ExpandProperty ConfiguredClockSpeed -First 1
$memoryTypeCode = $memoryModules | Where-Object { $_.SMBIOSMemoryType -and $_.SMBIOSMemoryType -ne 2 } | Select-Object -ExpandProperty SMBIOSMemoryType -First 1
if (-not $memoryTypeCode) {
  $memoryTypeCode = $memoryModules | Where-Object { $_.MemoryType -and $_.MemoryType -ne 2 } | Select-Object -ExpandProperty MemoryType -First 1
}
$memoryType = switch ([uint32]$memoryTypeCode) {
  20 { 'DDR' }
  21 { 'DDR2' }
  22 { 'DDR2 FB-DIMM' }
  24 { 'DDR3' }
  26 { 'DDR4' }
  27 { 'LPDDR' }
  28 { 'LPDDR2' }
  29 { 'LPDDR3' }
  30 { 'LPDDR4' }
  34 { 'DDR5' }
  35 { 'LPDDR5' }
  default { 'Not reported' }
}
$powerOutput = @(powercfg /getactivescheme 2>$null)
$powerPlan = 'Not reported'
if (($powerOutput -join ' ') -match '\((.+)\)') { $powerPlan = $Matches[1] }

$hasCpuTemperature = $null -ne $cpuTemperature
$hasGpuTelemetry = @($gpuDetails | Where-Object { $null -ne $_.temperature_c -or $null -ne $_.utilization_percent }).Count -gt 0
$sensorStatus = if ($hasCpuTemperature -and $hasGpuTelemetry) {
  "$sensorProvider CPU sensors and graphics-driver telemetry"
} elseif ($hasCpuTemperature) {
  "$sensorProvider CPU sensor telemetry"
} elseif ($hasGpuTelemetry) {
  'Graphics-driver telemetry and live Windows performance data'
} else {
  'Live Windows performance data'
}

[pscustomobject]@{
  cpu = [pscustomobject]@{
    name = [string]$cpu.Name.Trim()
    physical_cores = [uint32]$cpu.NumberOfCores
    logical_processors = [uint32]$cpu.NumberOfLogicalProcessors
    load_percent = $cpuLoad
    current_clock_mhz = $currentClock
    max_clock_mhz = if ($cpu.MaxClockSpeed) { [uint32]$cpu.MaxClockSpeed } else { $null }
    temperature_c = $cpuTemperature
  }
  gpus = @($gpuDetails)
  memory = [pscustomobject]@{
    total_mb = [uint64]0
    available_mb = [uint64]0
    module_count = [uint32]$memoryModules.Count
    memory_type = $memoryType
    configured_speed_mt_s = if ($moduleSpeed) { [uint32]$moduleSpeed } else { $null }
  }
  os_name = if ($os.Caption) { [string]$os.Caption } else { 'Windows' }
  os_build = if ($os.BuildNumber) { [string]$os.BuildNumber } else { 'Not reported' }
  active_power_plan = $powerPlan
  sensor_status = $sensorStatus
} | ConvertTo-Json -Depth 5 -Compress
"#;

    let raw = ps(script)?;
    let mut details: SystemDetails =
        serde_json::from_str(&raw).map_err(|e| format!("Could not parse system details: {e}"))?;
    details.memory.total_mb = total_mb;
    details.memory.available_mb = available_mb;
    Ok(details)
}

#[tauri::command]
async fn get_system_details() -> Result<SystemDetails, String> {
    tauri::async_runtime::spawn_blocking(read_system_details)
        .await
        .map_err(|error| format!("System-details worker stopped: {error}"))?
}

// ---- Reversible gaming session settings ------------------------------------

fn valid_guid(guid: &str) -> bool {
    if guid.len() != 36 {
        return false;
    }
    guid.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

/// Activates High Performance for the session. On systems where the built-in
/// plan is hidden, a deterministic temporary PicoBoost plan is created and its
/// GUID is recorded so restore can remove it instead of accumulating plans.
#[tauri::command]
fn optimize_power_plan() -> Result<PowerPlanState, String> {
    let raw = ps(
        "$active=@(powercfg /getactivescheme 2>&1); if($LASTEXITCODE -ne 0){ throw ($active -join ' ') }; \
         $orig=''; if(($active -join ' ') -match 'GUID:\\s+([a-f0-9\\-]+)'){ $orig=$Matches[1] }; \
         if(-not $orig){ throw 'Could not read the active power scheme' }; \
         $high='8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c'; \
         $pico='ce7f7cf4-35ca-4c0d-bf54-85b9a6e822d6'; \
         $all=@(powercfg /l 2>&1); $target=$null; $created=$null; \
         if(($all -join ' ') -match $high){ $target=$high } \
         elseif(($all -join ' ') -match $pico){ $target=$pico } \
         else { \
           $duplicate=@(powercfg -duplicatescheme $high $pico 2>&1); \
           if($LASTEXITCODE -ne 0){ throw ($duplicate -join ' ') }; \
           powercfg /changename $pico 'PicoBoost Performance' 'Temporary reversible gaming session plan' 2>&1 | Out-Null; \
           $target=$pico; $created=$pico \
         }; \
         $set=@(powercfg /setactive $target 2>&1); if($LASTEXITCODE -ne 0){ throw ($set -join ' ') }; \
         $verify=@(powercfg /getactivescheme 2>&1); if($LASTEXITCODE -ne 0 -or -not (($verify -join ' ') -match $target)){ throw 'Power scheme change could not be verified' }; \
         [pscustomobject]@{ original_guid=$orig; created_guid=$created } | ConvertTo-Json -Compress",
    )?;
    serde_json::from_str(&raw).map_err(|e| format!("Could not parse power-plan state: {e}"))
}

#[tauri::command]
fn restore_power_plan(state: PowerPlanState) -> Result<(), String> {
    if !valid_guid(&state.original_guid) {
        return Err("Invalid original power scheme GUID in restore state".into());
    }
    if let Some(created) = state.created_guid.as_deref() {
        if !valid_guid(created) {
            return Err("Invalid temporary power scheme GUID in restore state".into());
        }
    }

    let mut script = format!(
        "$out=@(powercfg /setactive '{}' 2>&1); if($LASTEXITCODE -ne 0){{ throw ($out -join ' ') }}; \
         $verify=@(powercfg /getactivescheme 2>&1); if($LASTEXITCODE -ne 0 -or -not (($verify -join ' ') -match '{}')){{ throw 'Power scheme restore could not be verified' }};",
        state.original_guid, state.original_guid
    );
    if let Some(created) = state.created_guid {
        if created != state.original_guid {
            script.push_str(&format!(
                " $delete=@(powercfg /delete '{}' 2>&1); if($LASTEXITCODE -ne 0){{ throw ($delete -join ' ') }};",
                created
            ));
        }
    }
    ps(&script)?;
    Ok(())
}

/// Snapshots and applies the two documented per-user Windows gaming settings
/// used by PicoBoost. If either write fails, the script rolls both values back
/// before returning an error.
#[tauri::command]
fn apply_windows_gaming_settings(
    enable_game_mode: bool,
    pause_background_recording: bool,
) -> Result<WindowsGamingState, String> {
    let script = r#"
$enableGameMode = __ENABLE_GAME_MODE__
$pauseRecording = __PAUSE_RECORDING__
$gameModePath = 'HKCU:\Software\Microsoft\GameBar'
$gameModeName = 'AutoGameModeEnabled'
$recordingPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\GameDVR'
$recordingName = 'VKMSaveHistoricalVideo'

function Get-DwordState([string]$path, [string]$name) {
  if (Test-Path -LiteralPath $path) {
    $item = Get-ItemProperty -LiteralPath $path -ErrorAction Stop
    $property = $item.PSObject.Properties[$name]
    if ($property) { return [pscustomobject]@{ existed=$true; value=[uint32]$property.Value } }
  }
  return [pscustomobject]@{ existed=$false; value=$null }
}
function Set-Dword([string]$path, [string]$name, [uint32]$value) {
  New-Item -Path $path -Force | Out-Null
  New-ItemProperty -LiteralPath $path -Name $name -PropertyType DWord -Value $value -Force | Out-Null
}
function Restore-Dword([string]$path, [string]$name, $state) {
  if ($null -eq $state) { return }
  if ($state.existed) { Set-Dword $path $name ([uint32]$state.value) }
  elseif (Test-Path -LiteralPath $path) { Remove-ItemProperty -LiteralPath $path -Name $name -ErrorAction SilentlyContinue }
}

$gameModeState = if ($enableGameMode) { Get-DwordState $gameModePath $gameModeName } else { $null }
$recordingState = if ($pauseRecording) { Get-DwordState $recordingPath $recordingName } else { $null }
try {
  if ($enableGameMode) { Set-Dword $gameModePath $gameModeName 1 }
  if ($pauseRecording) { Set-Dword $recordingPath $recordingName 0 }
} catch {
  Restore-Dword $gameModePath $gameModeName $gameModeState
  Restore-Dword $recordingPath $recordingName $recordingState
  throw
}
[pscustomobject]@{
  game_mode_enabled=$gameModeState
  historical_video_enabled=$recordingState
} | ConvertTo-Json -Depth 4 -Compress
"#
    .replace(
        "__ENABLE_GAME_MODE__",
        if enable_game_mode { "$true" } else { "$false" },
    )
    .replace(
        "__PAUSE_RECORDING__",
        if pause_background_recording {
            "$true"
        } else {
            "$false"
        },
    );

    let raw = ps(&script)?;
    serde_json::from_str(&raw).map_err(|e| format!("Could not parse gaming-settings state: {e}"))
}

fn restore_dword_statement(
    path: &str,
    name: &str,
    state: &RegistryDwordState,
) -> Result<String, String> {
    if state.existed {
        let value = state
            .value
            .ok_or_else(|| format!("Missing original value for {name}"))?;
        Ok(format!(
            "New-Item -Path '{path}' -Force | Out-Null; New-ItemProperty -LiteralPath '{path}' -Name '{name}' -PropertyType DWord -Value {value} -Force | Out-Null;"
        ))
    } else {
        Ok(format!(
            "if(Test-Path -LiteralPath '{path}'){{ Remove-ItemProperty -LiteralPath '{path}' -Name '{name}' -ErrorAction SilentlyContinue }};"
        ))
    }
}

#[tauri::command]
fn restore_windows_gaming_settings(state: WindowsGamingState) -> Result<(), String> {
    let mut script = String::new();
    if let Some(snapshot) = state.game_mode_enabled.as_ref() {
        script.push_str(&restore_dword_statement(
            "HKCU:\\Software\\Microsoft\\GameBar",
            "AutoGameModeEnabled",
            snapshot,
        )?);
    }
    if let Some(snapshot) = state.historical_video_enabled.as_ref() {
        script.push_str(&restore_dword_statement(
            "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\GameDVR",
            "VKMSaveHistoricalVideo",
            snapshot,
        )?);
    }
    if !script.is_empty() {
        ps(&script)?;
    }
    Ok(())
}

// ---- Legacy service recovery ----------------------------------------------

/// Restarts services that PicoBoost paused during a boost and returns the names
/// confirmed running. Failed names remain in the frontend restore snapshot.
#[tauri::command]
fn start_services(names: Vec<String>) -> Result<Vec<String>, String> {
    const LEGACY_SERVICES: &[&str] = &[
        "Spooler",
        "WSearch",
        "SysMain",
        "wuauserv",
        "bits",
        "DiagTrack",
        "MapsBroker",
        "Fax",
    ];
    let names: Vec<String> = names
        .into_iter()
        .filter(|name| {
            LEGACY_SERVICES
                .iter()
                .any(|allowed| name.eq_ignore_ascii_case(allowed))
        })
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let script = format!(
        "$svcs={}; foreach($s in $svcs){{ $svc=Get-Service -Name $s -ErrorAction SilentlyContinue; \
         if($svc){{ try{{ if($svc.Status -ne 'Running'){{ Start-Service -Name $s -ErrorAction Stop; \
         $svc.WaitForStatus('Running',[TimeSpan]::FromSeconds(8)); $svc.Refresh() }}; if($svc.Status -eq 'Running'){{ $s }} }}catch{{}} }} }}",
        ps_array(&names)
    );
    let out = ps(&script)?;
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

// ---- Explicit system cleanup -----------------------------------------------

const ONE_DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let key = path.to_string_lossy().to_ascii_lowercase();
    if !paths
        .iter()
        .any(|p| p.to_string_lossy().to_ascii_lowercase() == key)
    {
        paths.push(path);
    }
}

fn browser_cache_paths(local: &Path) -> Vec<PathBuf> {
    let roots = [
        local.join("Google\\Chrome\\User Data"),
        local.join("Microsoft\\Edge\\User Data"),
        local.join("BraveSoftware\\Brave-Browser\\User Data"),
    ];
    let mut paths = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name != "Default" && name != "Guest Profile" && !name.starts_with("Profile ") {
                continue;
            }
            for child in ["Cache", "Code Cache", "GPUCache"] {
                push_unique(&mut paths, entry.path().join(child));
            }
        }
    }
    paths
}

fn cleanup_specs() -> Vec<CleanupSpec> {
    let local = env_path("LOCALAPPDATA");
    let profile = env_path("USERPROFILE");
    let windows = env_path("WINDIR");
    let mut temp_paths = Vec::new();
    if let Some(path) = env_path("TEMP") {
        push_unique(&mut temp_paths, path);
    }
    if let Some(path) = env_path("TMP") {
        push_unique(&mut temp_paths, path);
    }

    let mut nuget_http = Vec::new();
    if let Some(base) = &local {
        push_unique(&mut nuget_http, base.join("NuGet\\v3-cache"));
        push_unique(&mut nuget_http, base.join("NuGet\\http-cache"));
    }
    for temp in &temp_paths {
        let Ok(entries) = fs::read_dir(temp) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("NuGetScratch") {
                push_unique(&mut nuget_http, entry.path());
            }
        }
    }

    let mut shader_paths = Vec::new();
    if let Some(base) = &local {
        for suffix in [
            "NVIDIA\\DXCache",
            "NVIDIA\\GLCache",
            "NVIDIA Corporation\\NV_Cache",
            "AMD\\DxCache",
            "AMD\\GLCache",
            "D3DSCache",
        ] {
            push_unique(&mut shader_paths, base.join(suffix));
        }
    }

    vec![
        CleanupSpec {
            id: "user_temp",
            name: "User temporary files",
            description: "Temporary files untouched for at least 24 hours.",
            group: "Everyday",
            default_selected: true,
            caution: None,
            min_age: Some(ONE_DAY),
            paths: temp_paths,
            recycle_bin: false,
        },
        CleanupSpec {
            id: "crash_dumps",
            name: "Application crash dumps",
            description: "Diagnostic dumps left behind by crashed applications.",
            group: "Everyday",
            default_selected: true,
            caution: Some("Keep these if you are currently diagnosing a crash."),
            min_age: None,
            paths: local
                .as_ref()
                .map(|p| vec![p.join("CrashDumps")])
                .unwrap_or_default(),
            recycle_bin: false,
        },
        CleanupSpec {
            id: "browser_cache",
            name: "Browser cache",
            description: "Cached page resources from Chrome, Edge, and Brave profiles.",
            group: "Everyday",
            default_selected: false,
            caution: Some("Browsers may reload websites more slowly once. History, cookies, and passwords are never touched."),
            min_age: None,
            paths: local
                .as_ref()
                .map(|p| browser_cache_paths(p))
                .unwrap_or_default(),
            recycle_bin: false,
        },
        CleanupSpec {
            id: "recycle_bin",
            name: "Recycle Bin",
            description: "Items you previously moved to the Windows Recycle Bin.",
            group: "Everyday",
            default_selected: false,
            caution: Some("Permanent: emptied items cannot be restored from the Recycle Bin."),
            min_age: None,
            paths: Vec::new(),
            recycle_bin: true,
        },
        CleanupSpec {
            id: "pip_cache",
            name: "pip download cache",
            description: "Downloaded Python wheels and package archives.",
            group: "Developer",
            default_selected: false,
            caution: Some("pip will download removed packages again when needed."),
            min_age: None,
            paths: {
                let mut paths = Vec::new();
                if let Some(base) = &local { push_unique(&mut paths, base.join("pip\\Cache")); }
                if let Some(base) = &profile { push_unique(&mut paths, base.join(".cache\\pip")); }
                paths
            },
            recycle_bin: false,
        },
        CleanupSpec {
            id: "nuget_http",
            name: "NuGet download cache",
            description: "NuGet HTTP cache and temporary extraction files.",
            group: "Developer",
            default_selected: false,
            caution: Some("NuGet may download package data again."),
            min_age: None,
            paths: nuget_http,
            recycle_bin: false,
        },
        CleanupSpec {
            id: "npm_cache",
            name: "npm cache",
            description: "npm's downloaded package content and logs.",
            group: "Developer",
            default_selected: false,
            caution: Some("npm will download removed package content again."),
            min_age: None,
            paths: local
                .as_ref()
                .map(|p| vec![p.join("npm-cache\\_cacache"), p.join("npm-cache\\_logs")])
                .unwrap_or_default(),
            recycle_bin: false,
        },
        CleanupSpec {
            id: "nuget_packages",
            name: "NuGet global packages",
            description: "Installed package copies shared by .NET projects.",
            group: "Advanced",
            default_selected: false,
            caution: Some("Large cleanup: projects must restore packages again and offline builds may fail."),
            min_age: None,
            paths: profile
                .as_ref()
                .map(|p| vec![p.join(".nuget\\packages")])
                .unwrap_or_default(),
            recycle_bin: false,
        },
        CleanupSpec {
            id: "shader_cache",
            name: "Graphics shader caches",
            description: "DirectX, NVIDIA, and AMD compiled shader caches.",
            group: "Advanced",
            default_selected: false,
            caution: Some("Games can stutter temporarily while shaders are compiled again."),
            min_age: None,
            paths: shader_paths,
            recycle_bin: false,
        },
        CleanupSpec {
            id: "windows_temp",
            name: "Windows temporary files",
            description: "System temp files untouched for at least 24 hours; locked files are skipped.",
            group: "Advanced",
            default_selected: false,
            caution: Some("Some files require administrator access and will be safely skipped."),
            min_age: Some(ONE_DAY),
            paths: windows
                .as_ref()
                .map(|p| vec![p.join("Temp")])
                .unwrap_or_default(),
            recycle_bin: false,
        },
    ]
}

fn eligible(metadata: &fs::Metadata, min_age: Option<Duration>) -> bool {
    match min_age {
        None => true,
        Some(age) => metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|elapsed| elapsed >= age),
    }
}

fn is_safe_directory_root(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn scan_path(path: &Path, min_age: Option<Duration>, stats: &mut CleanupStats) {
    let Ok(root_meta) = fs::symlink_metadata(path) else {
        return;
    };
    if root_meta.file_type().is_symlink() {
        return;
    }
    if root_meta.is_file() {
        if eligible(&root_meta, min_age) {
            stats.files += 1;
            stats.bytes = stats.bytes.saturating_add(root_meta.len());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            scan_path(&entry.path(), min_age, stats);
        } else if kind.is_file() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if eligible(&metadata, min_age) {
                stats.files += 1;
                stats.bytes = stats.bytes.saturating_add(metadata.len());
            }
        }
    }
}

fn purge_path(path: &Path, min_age: Option<Duration>, remove_root: bool, stats: &mut CleanupStats) {
    let Ok(root_meta) = fs::symlink_metadata(path) else {
        return;
    };
    if root_meta.file_type().is_symlink() {
        return;
    }
    if root_meta.is_file() {
        if eligible(&root_meta, min_age) {
            let size = root_meta.len();
            match fs::remove_file(path) {
                Ok(()) => {
                    stats.files += 1;
                    stats.bytes = stats.bytes.saturating_add(size);
                }
                Err(_) => stats.failures += 1,
            }
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        stats.failures += 1;
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            stats.failures += 1;
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            purge_path(&entry.path(), min_age, true, stats);
        } else if kind.is_file() {
            let Ok(metadata) = entry.metadata() else {
                stats.failures += 1;
                continue;
            };
            if eligible(&metadata, min_age) {
                let size = metadata.len();
                match fs::remove_file(entry.path()) {
                    Ok(()) => {
                        stats.files += 1;
                        stats.bytes = stats.bytes.saturating_add(size);
                    }
                    Err(_) => stats.failures += 1,
                }
            }
        }
    }
    if remove_root {
        let _ = fs::remove_dir(path);
    }
}

#[cfg(windows)]
#[repr(C)]
struct RecycleBinInfo {
    cb_size: u32,
    size: i64,
    items: i64,
}

#[cfg(windows)]
#[link(name = "Shell32")]
unsafe extern "system" {
    fn SHQueryRecycleBinW(root_path: *const u16, info: *mut RecycleBinInfo) -> i32;
    fn SHEmptyRecycleBinW(window: *mut std::ffi::c_void, root_path: *const u16, flags: u32) -> i32;
}

#[cfg(windows)]
fn query_recycle_bin() -> CleanupStats {
    let mut info = RecycleBinInfo {
        cb_size: std::mem::size_of::<RecycleBinInfo>() as u32,
        size: 0,
        items: 0,
    };
    // SAFETY: null selects all drives and `info` has the documented layout/size.
    let result = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
    if result >= 0 {
        CleanupStats {
            files: info.items.max(0) as u64,
            bytes: info.size.max(0) as u64,
            failures: 0,
        }
    } else {
        CleanupStats::default()
    }
}

#[cfg(not(windows))]
fn query_recycle_bin() -> CleanupStats {
    CleanupStats::default()
}

#[cfg(windows)]
fn empty_recycle_bin() -> Result<(), String> {
    const SILENT_FLAGS: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;
    // SAFETY: null window/root are explicitly supported; flags suppress Shell UI.
    let result =
        unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), SILENT_FLAGS) };
    if result >= 0 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not empty the Recycle Bin (0x{:08X})",
            result as u32
        ))
    }
}

#[cfg(not(windows))]
fn empty_recycle_bin() -> Result<(), String> {
    Err("Recycle Bin cleanup is available only on Windows".into())
}

#[tauri::command]
fn scan_cleanup() -> Vec<CleanupCategory> {
    cleanup_specs()
        .into_iter()
        .map(|spec| {
            let mut stats = if spec.recycle_bin {
                query_recycle_bin()
            } else {
                CleanupStats::default()
            };
            for path in &spec.paths {
                if is_safe_directory_root(path) {
                    scan_path(path, spec.min_age, &mut stats);
                }
            }
            CleanupCategory {
                id: spec.id.into(),
                name: spec.name.into(),
                description: spec.description.into(),
                group: spec.group.into(),
                bytes: stats.bytes,
                files: stats.files,
                default_selected: spec.default_selected,
                caution: spec.caution.map(str::to_string),
                available: spec.recycle_bin || !spec.paths.is_empty(),
            }
        })
        .collect()
}

#[tauri::command]
fn run_cleanup(ids: Vec<String>) -> Result<CleanupRunResult, String> {
    if ids.is_empty() {
        return Err("Select at least one cleanup category".into());
    }
    let requested: HashSet<&str> = ids.iter().map(String::as_str).collect();
    if requested.len() != ids.len() {
        return Err("Duplicate cleanup category".into());
    }
    let specs = cleanup_specs();
    let known: HashSet<&str> = specs.iter().map(|s| s.id).collect();
    if let Some(unknown) = requested.iter().find(|id| !known.contains(**id)) {
        return Err(format!("Unknown cleanup category: {unknown}"));
    }

    let mut total = CleanupStats::default();
    for spec in specs.into_iter().filter(|s| requested.contains(s.id)) {
        if spec.recycle_bin {
            let before = query_recycle_bin();
            match empty_recycle_bin() {
                Ok(()) => {
                    total.files += before.files;
                    total.bytes = total.bytes.saturating_add(before.bytes);
                }
                Err(_) => total.failures += 1,
            }
        } else {
            for path in &spec.paths {
                // Category roots remain intact; only eligible contents are removed.
                if is_safe_directory_root(path) {
                    purge_path(path, spec.min_age, false, &mut total);
                }
            }
        }
    }

    Ok(CleanupRunResult {
        files_removed: total.files,
        bytes_freed: total.bytes,
        failed_items: total.failures,
    })
}

// ---- Visual storage analyzer ----------------------------------------------

const STORAGE_CHILD_LIMIT: usize = 300;
// The frontend paints these on one canvas (not one DOM node per file), so a
// generous sample remains fast while producing a genuinely useful WizTree-like
// view. Remaining smaller files are represented as one honest aggregate block.
const STORAGE_LARGEST_LIMIT: usize = 1_200;
const STORAGE_DEPTH_LIMIT: usize = 128;

fn begin_storage_scan(inner: &mut StorageAnalyzerInner) -> (u64, Arc<StorageScanControl>) {
    if let Some(previous) = inner.scan_control.take() {
        previous.cancelled.store(true, Ordering::Release);
    }
    inner.generation = inner.generation.wrapping_add(1);
    let control = Arc::new(StorageScanControl::default());
    inner.scan_control = Some(Arc::clone(&control));
    (inner.generation, control)
}

fn ensure_storage_scan_active(control: &StorageScanControl) -> Result<(), String> {
    if control.cancelled.load(Ordering::Acquire) {
        return Err("Storage scan cancelled".into());
    }
    Ok(())
}

fn storage_scan_checkpoint(control: &StorageScanControl) -> Result<(), String> {
    ensure_storage_scan_active(control)?;
    // Large directory walks can otherwise monopolize a CPU core and saturate
    // the filesystem queue. Yield occasionally so window/input threads remain
    // responsive without materially slowing normal folder scans.
    if control
        .visited
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(512)
    {
        std::thread::yield_now();
    }
    Ok(())
}

fn modified_milliseconds(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn display_storage_path(path: &Path) -> String {
    let rendered = path.to_string_lossy();
    if let Some(network_path) = rendered.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{network_path}")
    } else {
        rendered.trim_start_matches(r"\\?\").to_string()
    }
}

fn shell_storage_path(path: &Path) -> PathBuf {
    PathBuf::from(display_storage_path(path))
}

fn build_storage_index(root: &Path, control: &StorageScanControl) -> Result<StorageIndex, String> {
    storage_scan_checkpoint(control)?;
    if !is_safe_directory_root(root) {
        return Err("The selected storage folder is unavailable or is a link".into());
    }
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("Could not open scan root: {error}"))?;
    let started = Instant::now();
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .clamp(2, 8);
    control
        .workers
        .store(worker_count as u64, Ordering::Relaxed);
    let walker = jwalk::WalkDir::new(&canonical_root)
        .min_depth(1)
        .max_depth(STORAGE_DEPTH_LIMIT)
        .skip_hidden(false)
        .follow_links(false)
        .parallelism(jwalk::Parallelism::RayonNewPool(worker_count))
        .try_into_iter()
        .map_err(|error| format!("Could not start indexed folder scan: {error}"))?;

    let mut nodes = Vec::<StorageIndexNode>::new();
    let mut directory_stack = Vec::<usize>::new();
    let mut skipped = 0u64;
    for entry in walker {
        storage_scan_checkpoint(control)?;
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                skipped = skipped.saturating_add(1);
                continue;
            }
        };
        let depth = entry.depth();
        while directory_stack.len() >= depth {
            if let Some(closing) = directory_stack.pop() {
                nodes[closing].subtree_end = nodes.len();
            }
        }
        if entry.path_is_symlink() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        if entry.read_children_error.is_some() {
            skipped = skipped.saturating_add(1);
        }
        let file_type = entry.file_type();
        if !file_type.is_dir() && !file_type.is_file() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => Some(metadata),
            Err(_) if file_type.is_file() => {
                skipped = skipped.saturating_add(1);
                continue;
            }
            Err(_) => None,
        };
        let bytes = metadata.as_ref().map_or(0, fs::Metadata::len);
        let parent = directory_stack.last().copied();
        let node_index = nodes.len();
        nodes.push(StorageIndexNode {
            parent,
            name: entry.file_name().to_string_lossy().to_string(),
            is_directory: file_type.is_dir(),
            bytes,
            files: u64::from(file_type.is_file()),
            folders: 0,
            modified_ms: metadata.as_ref().and_then(modified_milliseconds),
            subtree_end: node_index + 1,
        });
        if file_type.is_dir() {
            directory_stack.push(node_index);
            if depth == STORAGE_DEPTH_LIMIT {
                skipped = skipped.saturating_add(1);
            }
        }
    }
    while let Some(closing) = directory_stack.pop() {
        nodes[closing].subtree_end = nodes.len();
    }

    for node_index in (0..nodes.len()).rev() {
        let Some(parent) = nodes[node_index].parent else {
            continue;
        };
        let child_bytes = nodes[node_index].bytes;
        let child_files = nodes[node_index].files;
        let child_folders = nodes[node_index]
            .folders
            .saturating_add(u64::from(nodes[node_index].is_directory));
        nodes[parent].bytes = nodes[parent].bytes.saturating_add(child_bytes);
        nodes[parent].files = nodes[parent].files.saturating_add(child_files);
        nodes[parent].folders = nodes[parent].folders.saturating_add(child_folders);
    }
    let mut total_bytes = 0u64;
    let mut files = 0u64;
    let mut folders = 0u64;
    for node in nodes.iter().filter(|node| node.parent.is_none()) {
        total_bytes = total_bytes.saturating_add(node.bytes);
        files = files.saturating_add(node.files);
        folders = folders.saturating_add(node.folders.saturating_add(u64::from(node.is_directory)));
    }
    Ok(StorageIndex {
        nodes,
        total_bytes,
        files,
        folders,
        skipped,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        scan_mode: "Parallel index".into(),
    })
}

// Raw Windows volume handles only accept sector-aligned I/O. Keeping this
// adapter local avoids making the rest of the storage index aware of that
// platform detail while still allowing the NTFS parser to seek normally.
#[cfg(windows)]
struct SectorReader<R: Read + Seek> {
    inner: R,
    sector_size: usize,
    position: u64,
    buffer: Vec<u8>,
}

#[cfg(windows)]
impl<R: Read + Seek> SectorReader<R> {
    fn new(inner: R, sector_size: usize) -> Result<Self, String> {
        if !sector_size.is_power_of_two() {
            return Err("The NTFS volume reported an invalid sector size".into());
        }
        Ok(Self {
            inner,
            sector_size,
            position: 0,
            buffer: Vec::new(),
        })
    }

    fn align_down(&self, value: u64) -> u64 {
        value / self.sector_size as u64 * self.sector_size as u64
    }

    fn align_up(&self, value: usize) -> usize {
        value.div_ceil(self.sector_size) * self.sector_size
    }
}

#[cfg(windows)]
impl<R: Read + Seek> Read for SectorReader<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let aligned_position = self.align_down(self.position);
        let offset = (self.position - aligned_position) as usize;
        let read_len = self.align_up(offset.saturating_add(output.len()));
        self.inner.seek(SeekFrom::Start(aligned_position))?;
        self.buffer.resize(read_len, 0);
        self.inner.read_exact(&mut self.buffer)?;
        output.copy_from_slice(&self.buffer[offset..offset + output.len()]);
        self.position = self.position.saturating_add(output.len() as u64);
        Ok(output.len())
    }
}

#[cfg(windows)]
impl<R: Read + Seek> Seek for SectorReader<R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let next = match position {
            SeekFrom::Start(value) => Some(value),
            SeekFrom::Current(value) if value >= 0 => self.position.checked_add(value as u64),
            SeekFrom::Current(value) => self.position.checked_sub(value.unsigned_abs()),
            SeekFrom::End(_) => None,
        }
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid raw-volume seek")
        })?;
        self.position = next;
        Ok(next)
    }
}

#[cfg(windows)]
fn ntfs_volume_path(root: &Path) -> Result<(String, Vec<String>), String> {
    let path = display_storage_path(root).replace('/', "\\");
    let bytes = path.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err("Fast NTFS mode supports local drive-letter folders only".into());
    }
    let drive = (bytes[0] as char).to_ascii_uppercase();
    let components = path[3..]
        .split('\\')
        .filter(|component| !component.is_empty())
        .map(str::to_string)
        .collect();
    Ok((format!(r"\\.\{drive}:"), components))
}

#[cfg(windows)]
fn ntfs_time_milliseconds(time: ntfs::NtfsTime) -> Option<u64> {
    const NTFS_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
    time.nt_timestamp()
        .checked_sub(NTFS_UNIX_EPOCH)
        .map(|intervals| intervals / 10_000)
}

#[cfg(windows)]
struct MftStorageRecord {
    number: u64,
    parent: u64,
    name: String,
    is_directory: bool,
    bytes: u64,
    modified_ms: Option<u64>,
}

#[cfg(windows)]
fn fixup_mft_record_parallel(record_number: u64, data: &mut [u8]) -> Result<(), String> {
    use ntfs_reader::api::{NtfsFileRecordHeader, SECTOR_SIZE};

    if data.len() < std::mem::size_of::<NtfsFileRecordHeader>() {
        return Err(format!("MFT record {record_number} is truncated"));
    }
    // NTFS record headers are packed and may not be naturally aligned inside
    // the volume buffer, so copy the header before reading its fields.
    let header = unsafe { std::ptr::read_unaligned(data.as_ptr().cast::<NtfsFileRecordHeader>()) };
    let usn_start = header.update_sequence_offset as usize;
    let usa_start = usn_start.saturating_add(2);
    let usa_end = usn_start.saturating_add(header.update_sequence_length as usize * 2);
    if usn_start + 2 > data.len() || usa_end > data.len() {
        return Err(format!(
            "MFT record {record_number} has an invalid fixup table"
        ));
    }
    let signature = [data[usn_start], data[usn_start + 1]];
    let mut sector_end = SECTOR_SIZE - 2;
    for replacement in (usa_start..usa_end).step_by(2) {
        if sector_end + 2 > data.len() {
            break;
        }
        if data[sector_end..sector_end + 2] != signature {
            return Err(format!(
                "MFT record {record_number} failed its integrity check"
            ));
        }
        let value = [data[replacement], data[replacement + 1]];
        data[sector_end..sector_end + 2].copy_from_slice(&value);
        sector_end += SECTOR_SIZE;
    }
    Ok(())
}

#[cfg(windows)]
fn load_parallel_mft(volume: ntfs_reader::volume::Volume) -> Result<ntfs_reader::mft::Mft, String> {
    use ntfs_reader::aligned_reader::open_volume;
    use ntfs_reader::api::NtfsAttributeType;
    use ntfs_reader::mft::Mft;
    use rayon::prelude::*;

    let mut reader = open_volume(&volume.path)
        .map_err(|error| format!("Could not open the NTFS record stream: {error}"))?;
    let mft_record = Mft::get_record_fs(&mut reader, volume.file_record_size, volume.mft_position)
        .map_err(|error| format!("Could not read the MFT header: {error}"))?;
    let mut data = Mft::read_data_fs(&volume, &mut reader, &mft_record, NtfsAttributeType::Data)
        .map_err(|error| format!("Could not stream the MFT data: {error}"))?
        .ok_or("The NTFS volume has no MFT data stream")?;
    let bitmap = Mft::read_data_fs(&volume, &mut reader, &mft_record, NtfsAttributeType::Bitmap)
        .map_err(|error| format!("Could not read the MFT allocation bitmap: {error}"))?
        .ok_or("The NTFS volume has no MFT allocation bitmap")?;
    let record_size = usize::try_from(volume.file_record_size)
        .map_err(|_| "The NTFS record size is not supported")?;
    data.par_chunks_exact_mut(record_size)
        .enumerate()
        .try_for_each(|(number, record)| fixup_mft_record_parallel(number as u64, record))?;
    let max_record = data.len() as u64 / volume.file_record_size;
    Ok(Mft {
        volume,
        data,
        bitmap,
        max_record,
    })
}

#[cfg(windows)]
fn build_mft_stream_storage_index(
    root: &Path,
    control: &StorageScanControl,
) -> Result<StorageIndex, String> {
    use ntfs_reader::api::{ntfs_to_unix_time, NtfsAttributeType};
    use ntfs_reader::volume::Volume;
    use rayon::prelude::*;

    const ROOT_RECORD: u64 = 5;
    let started = Instant::now();
    let (volume_path, components) = ntfs_volume_path(root)?;
    control.workers.store(
        rayon::current_num_threads().min(8) as u64,
        Ordering::Relaxed,
    );
    ensure_storage_scan_active(control)?;
    let volume = Volume::new(&volume_path)
        .map_err(|error| format!("Could not open the NTFS volume metadata: {error}"))?;
    let mft = load_parallel_mft(volume)?;

    let records = (16..mft.max_record)
        .into_par_iter()
        .filter_map(|number| {
            control.visited.fetch_add(1, Ordering::Relaxed);
            if control.cancelled.load(Ordering::Acquire) || !mft.record_exists(number) {
                return None;
            }
            let file = mft.get_record(number)?;
            if !file.is_used() {
                return None;
            }
            let file_name = file.get_best_file_name(&mft)?;
            let name = file_name.to_string();
            if name == "." || name == ".." {
                return None;
            }
            let mut bytes = 0u64;
            let mut modified_ms = None;
            file.attributes(|attribute| {
                if attribute.header.type_id == NtfsAttributeType::Data as u32 {
                    if attribute.header.is_non_resident == 0 {
                        if let Some(header) = attribute.resident_header() {
                            bytes = bytes.max(header.value_length as u64);
                        }
                    } else if let Some(header) = attribute.nonresident_header() {
                        bytes = bytes.max(header.data_size);
                    }
                } else if attribute.header.type_id == NtfsAttributeType::StandardInformation as u32
                {
                    if let Some(info) = attribute.as_standard_info() {
                        let timestamp =
                            ntfs_to_unix_time(info.modification_time).unix_timestamp_nanos();
                        if timestamp >= 0 {
                            modified_ms =
                                Some((timestamp / 1_000_000).min(u64::MAX as i128) as u64);
                        }
                    }
                }
            });
            Some(MftStorageRecord {
                number: file.number(),
                parent: file_name.parent(),
                name,
                is_directory: file.is_directory(),
                bytes,
                modified_ms,
            })
        })
        .collect::<Vec<_>>();
    ensure_storage_scan_active(control)?;

    let mut child_lookup = HashMap::<u64, Vec<usize>>::new();
    for (record_index, record) in records.iter().enumerate() {
        child_lookup
            .entry(record.parent)
            .or_default()
            .push(record_index);
    }

    let mut selected_record = ROOT_RECORD;
    for component in components {
        let component = component.to_ascii_lowercase();
        selected_record = child_lookup
            .get(&selected_record)
            .and_then(|children| {
                children.iter().find_map(|record_index| {
                    let record = &records[*record_index];
                    (record.is_directory && record.name.to_ascii_lowercase() == component)
                        .then_some(record.number)
                })
            })
            .ok_or_else(|| format!("The selected folder {component} was not found in the MFT"))?;
    }

    enum MftVisit {
        Enter(usize, Option<usize>, usize),
        Exit(usize),
    }
    let mut nodes = Vec::<StorageIndexNode>::new();
    let mut skipped = 0u64;
    let mut visits = Vec::<MftVisit>::new();
    if let Some(children) = child_lookup.get(&selected_record) {
        for record_index in children.iter().rev() {
            visits.push(MftVisit::Enter(*record_index, None, 1));
        }
    }
    while let Some(visit) = visits.pop() {
        match visit {
            MftVisit::Exit(node_index) => nodes[node_index].subtree_end = nodes.len(),
            MftVisit::Enter(record_index, parent, depth) => {
                ensure_storage_scan_active(control)?;
                let record = &records[record_index];
                let node_index = nodes.len();
                nodes.push(StorageIndexNode {
                    parent,
                    name: record.name.clone(),
                    is_directory: record.is_directory,
                    bytes: if record.is_directory { 0 } else { record.bytes },
                    files: u64::from(!record.is_directory),
                    folders: 0,
                    modified_ms: record.modified_ms,
                    subtree_end: node_index + 1,
                });
                if record.is_directory {
                    visits.push(MftVisit::Exit(node_index));
                    if depth >= STORAGE_DEPTH_LIMIT {
                        skipped = skipped.saturating_add(1);
                    } else if let Some(children) = child_lookup.get(&record.number) {
                        for child in children.iter().rev() {
                            visits.push(MftVisit::Enter(*child, Some(node_index), depth + 1));
                        }
                    }
                }
            }
        }
    }
    for node_index in (0..nodes.len()).rev() {
        let Some(parent) = nodes[node_index].parent else {
            continue;
        };
        let child_bytes = nodes[node_index].bytes;
        let child_files = nodes[node_index].files;
        let child_folders = nodes[node_index]
            .folders
            .saturating_add(u64::from(nodes[node_index].is_directory));
        nodes[parent].bytes = nodes[parent].bytes.saturating_add(child_bytes);
        nodes[parent].files = nodes[parent].files.saturating_add(child_files);
        nodes[parent].folders = nodes[parent].folders.saturating_add(child_folders);
    }
    let mut total_bytes = 0u64;
    let mut files = 0u64;
    let mut folders = 0u64;
    for node in nodes.iter().filter(|node| node.parent.is_none()) {
        total_bytes = total_bytes.saturating_add(node.bytes);
        files = files.saturating_add(node.files);
        folders = folders.saturating_add(node.folders.saturating_add(u64::from(node.is_directory)));
    }
    Ok(StorageIndex {
        nodes,
        total_bytes,
        files,
        folders,
        skipped,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        scan_mode: "MFT stream".into(),
    })
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn walk_ntfs_directory<'n, T: Read + Seek>(
    ntfs: &'n ntfs::Ntfs,
    filesystem: &mut T,
    directory: &ntfs::NtfsFile<'n>,
    parent: Option<usize>,
    depth: usize,
    nodes: &mut Vec<StorageIndexNode>,
    seen_files: &mut HashSet<u64>,
    skipped: &mut u64,
    control: &StorageScanControl,
) -> Result<(), String> {
    use ntfs::structured_values::{NtfsFileAttributeFlags, NtfsFileNamespace};

    ensure_storage_scan_active(control)?;
    let index = match directory.directory_index(filesystem) {
        Ok(index) => index,
        Err(_) => {
            *skipped = skipped.saturating_add(1);
            return Ok(());
        }
    };
    let mut entries = index.entries();
    while let Some(entry_result) = entries.next(filesystem) {
        storage_scan_checkpoint(control)?;
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => {
                *skipped = skipped.saturating_add(1);
                continue;
            }
        };
        let file_name = match entry.key() {
            Some(Ok(file_name)) => file_name,
            _ => {
                *skipped = skipped.saturating_add(1);
                continue;
            }
        };
        // NTFS may index a second DOS 8.3 alias for the same file. Showing and
        // counting that alias would duplicate data in the map.
        if file_name.namespace() == NtfsFileNamespace::Dos {
            continue;
        }
        let name = file_name.name().to_string_lossy();
        if name == "." || name == ".." {
            continue;
        }
        let is_directory = file_name.is_directory();
        let record_number = entry.file_reference().file_record_number();
        let bytes = if !is_directory && seen_files.insert(record_number) {
            file_name.data_size()
        } else {
            0
        };
        let node_index = nodes.len();
        nodes.push(StorageIndexNode {
            parent,
            name,
            is_directory,
            bytes,
            files: u64::from(!is_directory),
            folders: 0,
            modified_ms: ntfs_time_milliseconds(file_name.modification_time()),
            subtree_end: node_index + 1,
        });

        if is_directory {
            let is_reparse_point = file_name
                .file_attributes()
                .contains(NtfsFileAttributeFlags::REPARSE_POINT);
            if depth >= STORAGE_DEPTH_LIMIT || is_reparse_point {
                *skipped = skipped.saturating_add(1);
            } else {
                match entry.to_file(ntfs, filesystem) {
                    Ok(child) => walk_ntfs_directory(
                        ntfs,
                        filesystem,
                        &child,
                        Some(node_index),
                        depth + 1,
                        nodes,
                        seen_files,
                        skipped,
                        control,
                    )?,
                    Err(_) => *skipped = skipped.saturating_add(1),
                }
            }
            nodes[node_index].subtree_end = nodes.len();
        }
    }
    Ok(())
}

#[cfg(windows)]
fn build_ntfs_storage_index(
    root: &Path,
    control: &StorageScanControl,
) -> Result<StorageIndex, String> {
    use ntfs::indexes::NtfsFileNameIndex;

    let started = Instant::now();
    let (volume, components) = ntfs_volume_path(root)?;
    let file = File::open(&volume).map_err(|error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            "Fast NTFS mode needs administrator access to read the volume metadata".to_string()
        } else {
            format!("Could not open the NTFS volume read-only: {error}")
        }
    })?;
    let sector_reader = SectorReader::new(file, 4096)?;
    let mut filesystem = BufReader::with_capacity(1024 * 1024, sector_reader);
    let mut ntfs = ntfs::Ntfs::new(&mut filesystem)
        .map_err(|error| format!("This drive is not a readable NTFS volume: {error}"))?;
    ntfs.read_upcase_table(&mut filesystem)
        .map_err(|error| format!("Could not read the NTFS name index: {error}"))?;
    let mut directory = ntfs
        .root_directory(&mut filesystem)
        .map_err(|error| format!("Could not open the NTFS root directory: {error}"))?;
    for component in components {
        ensure_storage_scan_active(control)?;
        let index = directory
            .directory_index(&mut filesystem)
            .map_err(|error| format!("Could not read {component}: {error}"))?;
        let mut finder = index.finder();
        let entry = NtfsFileNameIndex::find(&mut finder, &ntfs, &mut filesystem, &component)
            .ok_or_else(|| {
                format!("The selected folder {component} is not present in the NTFS index")
            })?
            .map_err(|error| format!("Could not locate {component} in the NTFS index: {error}"))?;
        let key = entry
            .key()
            .ok_or_else(|| format!("The NTFS entry for {component} has no name"))?
            .map_err(|error| format!("Could not read the NTFS name for {component}: {error}"))?;
        if !key.is_directory() {
            return Err(format!("{component} is not a folder"));
        }
        directory = entry
            .to_file(&ntfs, &mut filesystem)
            .map_err(|error| format!("Could not open {component} from the NTFS index: {error}"))?;
    }

    control.workers.store(1, Ordering::Relaxed);
    let mut nodes = Vec::new();
    let mut seen_files = HashSet::new();
    let mut skipped = 0u64;
    walk_ntfs_directory(
        &ntfs,
        &mut filesystem,
        &directory,
        None,
        0,
        &mut nodes,
        &mut seen_files,
        &mut skipped,
        control,
    )?;

    for node_index in (0..nodes.len()).rev() {
        let Some(parent) = nodes[node_index].parent else {
            continue;
        };
        let child_bytes = nodes[node_index].bytes;
        let child_files = nodes[node_index].files;
        let child_folders = nodes[node_index]
            .folders
            .saturating_add(u64::from(nodes[node_index].is_directory));
        nodes[parent].bytes = nodes[parent].bytes.saturating_add(child_bytes);
        nodes[parent].files = nodes[parent].files.saturating_add(child_files);
        nodes[parent].folders = nodes[parent].folders.saturating_add(child_folders);
    }
    let mut total_bytes = 0u64;
    let mut files = 0u64;
    let mut folders = 0u64;
    for node in nodes.iter().filter(|node| node.parent.is_none()) {
        total_bytes = total_bytes.saturating_add(node.bytes);
        files = files.saturating_add(node.files);
        folders = folders.saturating_add(node.folders.saturating_add(u64::from(node.is_directory)));
    }
    Ok(StorageIndex {
        nodes,
        total_bytes,
        files,
        folders,
        skipped,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        scan_mode: "Fast NTFS metadata".into(),
    })
}

#[cfg(not(windows))]
fn build_ntfs_storage_index(
    _root: &Path,
    _control: &StorageScanControl,
) -> Result<StorageIndex, String> {
    Err("Fast NTFS mode is available on Windows only".into())
}

fn build_best_storage_index(
    root: &Path,
    control: &StorageScanControl,
) -> Result<StorageIndex, String> {
    match build_preferred_ntfs_storage_index(root, control) {
        Ok(index) => Ok(index),
        Err(_) => build_storage_index(root, control),
    }
}

#[cfg(windows)]
fn build_preferred_ntfs_storage_index(
    root: &Path,
    control: &StorageScanControl,
) -> Result<StorageIndex, String> {
    let (_, components) = ntfs_volume_path(root)?;
    if components.is_empty() {
        build_mft_stream_storage_index(root, control)
            .or_else(|_| build_ntfs_storage_index(root, control))
    } else {
        // For a selected subfolder, following only that directory's NTFS index
        // touches dramatically fewer records than parsing the whole volume MFT.
        // A drive-root scan takes the opposite path and streams the MFT once.
        build_ntfs_storage_index(root, control)
            .or_else(|_| build_mft_stream_storage_index(root, control))
    }
}

#[cfg(not(windows))]
fn build_preferred_ntfs_storage_index(
    _root: &Path,
    _control: &StorageScanControl,
) -> Result<StorageIndex, String> {
    Err("Direct NTFS scanning is available on Windows only".into())
}

fn storage_index_relative_path(index: &StorageIndex, node_index: usize) -> PathBuf {
    let mut names = Vec::new();
    let mut cursor = Some(node_index);
    while let Some(current) = cursor {
        let Some(node) = index.nodes.get(current) else {
            break;
        };
        names.push(node.name.as_str());
        cursor = node.parent;
    }
    let mut relative = PathBuf::new();
    for name in names.into_iter().rev() {
        relative.push(name);
    }
    relative
}

fn storage_index_node_path(root: &Path, index: &StorageIndex, node_index: usize) -> PathBuf {
    root.join(storage_index_relative_path(index, node_index))
}

fn storage_index_item(
    root: &Path,
    index: &StorageIndex,
    node_index: usize,
    targets: &mut HashMap<String, StorageTarget>,
) -> Option<StorageItem> {
    let node = index.nodes.get(node_index)?;
    let path = storage_index_node_path(root, index, node_index);
    let id = format!("storage-index-{node_index}");
    targets.insert(
        id.clone(),
        StorageTarget {
            path,
            bytes: node.bytes,
            node_index: Some(node_index),
        },
    );
    Some(StorageItem {
        id,
        name: node.name.clone(),
        relative_path: storage_index_relative_path(index, node_index)
            .to_string_lossy()
            .to_string(),
        is_directory: node.is_directory,
        bytes: node.bytes,
        files: node.files,
        folders: node.folders,
        modified_ms: node.modified_ms,
    })
}

fn render_storage_index_view(
    root: &Path,
    index: &StorageIndex,
    current_node: Option<usize>,
) -> Result<(StorageScanResult, HashMap<String, StorageTarget>), String> {
    let current = match current_node {
        Some(node_index) => {
            let node = index
                .nodes
                .get(node_index)
                .ok_or("Indexed folder is no longer available")?;
            if !node.is_directory {
                return Err("Only indexed folders can be opened".into());
            }
            storage_index_node_path(root, index, node_index)
        }
        None => root.to_path_buf(),
    };
    let (total_bytes, files, folders, range) = match current_node {
        Some(node_index) => {
            let node = &index.nodes[node_index];
            (
                node.bytes,
                node.files,
                node.folders,
                (node_index + 1)..node.subtree_end,
            )
        }
        None => (
            index.total_bytes,
            index.files,
            index.folders,
            0..index.nodes.len(),
        ),
    };
    let mut child_indices: Vec<usize> = index
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(node_index, node)| (node.parent == current_node).then_some(node_index))
        .collect();
    child_indices.sort_by(|left, right| {
        index.nodes[*right]
            .bytes
            .cmp(&index.nodes[*left].bytes)
            .then_with(|| index.nodes[*left].name.cmp(&index.nodes[*right].name))
    });
    child_indices.truncate(STORAGE_CHILD_LIMIT);

    let mut largest = BinaryHeap::<Reverse<(u64, usize)>>::new();
    for node_index in range {
        let node = &index.nodes[node_index];
        if node.is_directory {
            continue;
        }
        if largest.len() < STORAGE_LARGEST_LIMIT {
            largest.push(Reverse((node.bytes, node_index)));
        } else if largest
            .peek()
            .is_some_and(|Reverse((smallest, _))| node.bytes > *smallest)
        {
            largest.pop();
            largest.push(Reverse((node.bytes, node_index)));
        }
    }
    let mut largest_indices: Vec<(u64, usize)> =
        largest.into_iter().map(|Reverse(entry)| entry).collect();
    largest_indices.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| index.nodes[left.1].name.cmp(&index.nodes[right.1].name))
    });

    let mut targets = HashMap::new();
    let children = child_indices
        .into_iter()
        .filter_map(|node_index| storage_index_item(root, index, node_index, &mut targets))
        .collect();
    let largest_files = largest_indices
        .into_iter()
        .filter_map(|(_, node_index)| storage_index_item(root, index, node_index, &mut targets))
        .collect();
    Ok((
        StorageScanResult {
            root: display_storage_path(root),
            current: display_storage_path(&current),
            total_bytes,
            files,
            folders,
            skipped: index.skipped,
            duration_ms: index.duration_ms,
            indexed_items: index.nodes.len() as u64,
            scan_mode: index.scan_mode.clone(),
            children,
            largest_files,
        },
        targets,
    ))
}

fn search_storage_index(
    session: &mut StorageSession,
    query: &str,
) -> Result<StorageSearchResult, String> {
    const SEARCH_RESULT_LIMIT: usize = 500;
    let query = query.trim();
    if query.is_empty() || query.len() > 200 {
        return Err("Enter between 1 and 200 characters to search the index".into());
    }
    let started = Instant::now();
    let terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    let search_paths = query.contains(['\\', '/']);
    let mut total_matches = 0u64;
    let mut best = BinaryHeap::<Reverse<(u64, usize)>>::new();
    for (node_index, node) in session.index.nodes.iter().enumerate() {
        let name = node.name.to_ascii_lowercase();
        let mut matches = terms.iter().all(|term| name.contains(term));
        if !matches && search_paths {
            let relative = storage_index_relative_path(&session.index, node_index)
                .to_string_lossy()
                .to_ascii_lowercase();
            matches = terms.iter().all(|term| relative.contains(term));
        }
        if !matches {
            continue;
        }
        total_matches = total_matches.saturating_add(1);
        if best.len() < SEARCH_RESULT_LIMIT {
            best.push(Reverse((node.bytes, node_index)));
        } else if best
            .peek()
            .is_some_and(|Reverse((smallest, _))| node.bytes > *smallest)
        {
            best.pop();
            best.push(Reverse((node.bytes, node_index)));
        }
    }
    let mut matches: Vec<(u64, usize)> = best.into_iter().map(|Reverse(entry)| entry).collect();
    matches.sort_by(|left, right| {
        right.0.cmp(&left.0).then_with(|| {
            session.index.nodes[left.1]
                .name
                .cmp(&session.index.nodes[right.1].name)
        })
    });
    let items = matches
        .into_iter()
        .filter_map(|(_, node_index)| {
            storage_index_item(
                &session.root,
                &session.index,
                node_index,
                &mut session.targets,
            )
        })
        .collect();
    Ok(StorageSearchResult {
        query: query.to_string(),
        total_matches,
        indexed_items: session.index.nodes.len() as u64,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        items,
    })
}

fn find_storage_index_node(index: &StorageIndex, relative: &Path) -> Option<usize> {
    let mut parent = None;
    for component in relative.components() {
        let name = component.as_os_str().to_string_lossy();
        parent = index
            .nodes
            .iter()
            .enumerate()
            .find_map(|(node_index, node)| {
                (node.parent == parent && node.name.eq_ignore_ascii_case(&name))
                    .then_some(node_index)
            });
        parent?;
    }
    parent
}

fn store_storage_index(
    state: &StorageAnalyzerState,
    generation: u64,
    root: PathBuf,
    current_node: Option<usize>,
    index: StorageIndex,
) -> Result<StorageScanResult, String> {
    let (scan, targets) = render_storage_index_view(&root, &index, current_node)?;
    let current = current_node.map_or_else(
        || root.clone(),
        |node_index| storage_index_node_path(&root, &index, node_index),
    );
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "Storage scan state is unavailable")?;
    if guard.generation != generation {
        return Err("Storage operation was superseded by a newer request".into());
    }
    guard.scan_control = None;
    guard.session = Some(StorageSession {
        root,
        current,
        current_node,
        targets,
        index,
    });
    Ok(scan)
}

async fn build_storage_index_background(
    root: PathBuf,
    control: Arc<StorageScanControl>,
) -> Result<StorageIndex, String> {
    tauri::async_runtime::spawn_blocking(move || build_best_storage_index(&root, &control))
        .await
        .map_err(|error| format!("Storage scan worker stopped: {error}"))?
}

#[tauri::command]
fn storage_fast_mode_support(path: String) -> StorageFastModeSupport {
    #[cfg(windows)]
    {
        let root = PathBuf::from(path.trim());
        let Ok((volume, _)) = ntfs_volume_path(&root) else {
            return StorageFastModeSupport {
                available: false,
                requires_elevation: false,
                volume: None,
                reason: "Fast mode supports local NTFS drive folders; the parallel index will be used here".into(),
            };
        };
        match File::open(&volume) {
            Ok(file) => {
                let support = SectorReader::new(file, 4096).and_then(|reader| {
                    let mut filesystem = BufReader::new(reader);
                    ntfs::Ntfs::new(&mut filesystem)
                        .map(|_| ())
                        .map_err(|error| format!("The selected drive is not readable as NTFS ({error})"))
                });
                match support {
                    Ok(()) => StorageFastModeSupport {
                        available: true,
                        requires_elevation: false,
                        volume: Some(volume),
                        reason: "Read-only NTFS metadata access is available".into(),
                    },
                    Err(reason) => StorageFastModeSupport {
                        available: false,
                        requires_elevation: false,
                        volume: Some(volume),
                        reason: format!("{reason}; using the safe parallel index"),
                    },
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => StorageFastModeSupport {
                available: false,
                requires_elevation: true,
                volume: Some(volume),
                reason: "A one-time Windows administrator approval unlocks the sequential MFT scanner; the main app remains non-admin".into(),
            },
            Err(error) => StorageFastModeSupport {
                available: false,
                requires_elevation: false,
                volume: Some(volume),
                reason: format!("Fast NTFS mode is unavailable ({error}); using the safe parallel index"),
            },
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        StorageFastModeSupport {
            available: false,
            requires_elevation: false,
            volume: None,
            reason: "Fast NTFS mode is available on Windows only".into(),
        }
    }
}

#[tauri::command]
fn cancel_storage_scan(state: tauri::State<'_, StorageAnalyzerState>) -> Result<(), String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "Storage scan state is unavailable")?;
    if let Some(control) = guard.scan_control.take() {
        control.cancelled.store(true, Ordering::Release);
    }
    guard.generation = guard.generation.wrapping_add(1);
    Ok(())
}

#[tauri::command]
fn get_storage_scan_progress(
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageScanProgress, String> {
    let control = state
        .inner
        .lock()
        .map_err(|_| "Storage scan state is unavailable")?
        .scan_control
        .clone();
    let Some(control) = control else {
        return Ok(StorageScanProgress {
            running: false,
            items_checked: 0,
            elapsed_ms: 0,
            workers: 0,
        });
    };
    Ok(StorageScanProgress {
        running: !control.cancelled.load(Ordering::Acquire),
        items_checked: control.visited.load(Ordering::Relaxed),
        elapsed_ms: control
            .started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
        workers: control.workers.load(Ordering::Relaxed),
    })
}

#[tauri::command]
async fn scan_storage_folder(
    path: String,
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageScanResult, String> {
    let root = fs::canonicalize(path.trim())
        .map_err(|e| format!("Could not open selected folder: {e}"))?;
    if !is_safe_directory_root(&root) {
        return Err("The selected storage folder is unavailable or is a link".into());
    }
    let (generation, control) = {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Storage scan state is unavailable")?;
        begin_storage_scan(&mut guard)
    };
    let index = build_storage_index_background(root.clone(), control).await?;
    store_storage_index(&state, generation, root, None, index)
}

#[tauri::command]
async fn scan_storage_folder_fast(
    path: String,
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageScanResult, String> {
    let root = fs::canonicalize(path.trim())
        .map_err(|error| format!("Could not open selected folder: {error}"))?;
    if !is_safe_directory_root(&root) {
        return Err("The selected storage folder is unavailable or is a link".into());
    }
    ntfs_volume_path(&root)?;
    let generation = {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Storage scan state is unavailable")?;
        let (generation, _) = begin_storage_scan(&mut guard);
        generation
    };
    let helper_root = root.clone();
    let index = tauri::async_runtime::spawn_blocking(move || {
        build_elevated_ntfs_storage_index(&helper_root)
    })
    .await
    .map_err(|error| format!("Elevated storage scanner stopped: {error}"))??;
    store_storage_index(&state, generation, root, None, index)
}

#[tauri::command]
async fn rescan_storage(
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageScanResult, String> {
    let (generation, root, current_relative, control) = {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Storage scan state is unavailable")?;
        let session = guard
            .session
            .as_ref()
            .ok_or("Choose a folder to analyze first")?;
        let root = session.root.clone();
        let current_relative = session
            .current
            .strip_prefix(&root)
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let (generation, control) = begin_storage_scan(&mut guard);
        (generation, root, current_relative, control)
    };
    let index = build_storage_index_background(root.clone(), control).await?;
    let current_node = if current_relative.as_os_str().is_empty() {
        None
    } else {
        find_storage_index_node(&index, &current_relative)
    };
    store_storage_index(&state, generation, root, current_node, index)
}

#[tauri::command]
fn browse_storage_item(
    id: String,
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageScanResult, String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "Storage scan state is unavailable")?;
    let session = guard
        .session
        .as_mut()
        .ok_or("Choose a folder to analyze first")?;
    let node_index = session
        .targets
        .get(&id)
        .and_then(|target| target.node_index)
        .ok_or("That item is no longer in the active index")?;
    let node = session
        .index
        .nodes
        .get(node_index)
        .ok_or("Indexed item is unavailable")?;
    if !node.is_directory {
        return Err("Only indexed folders can be opened".into());
    }
    let (scan, targets) =
        render_storage_index_view(&session.root, &session.index, Some(node_index))?;
    session.current_node = Some(node_index);
    session.current = storage_index_node_path(&session.root, &session.index, node_index);
    session.targets = targets;
    Ok(scan)
}

#[tauri::command]
fn storage_go_up(
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageScanResult, String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "Storage scan state is unavailable")?;
    let session = guard
        .session
        .as_mut()
        .ok_or("Choose a folder to analyze first")?;
    let parent = session
        .current_node
        .and_then(|node_index| session.index.nodes.get(node_index))
        .and_then(|node| node.parent);
    let (scan, targets) = render_storage_index_view(&session.root, &session.index, parent)?;
    session.current_node = parent;
    session.current = parent.map_or_else(
        || session.root.clone(),
        |node_index| storage_index_node_path(&session.root, &session.index, node_index),
    );
    session.targets = targets;
    Ok(scan)
}

#[tauri::command]
fn search_storage(
    query: String,
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageSearchResult, String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "Storage scan state is unavailable")?;
    let session = guard
        .session
        .as_mut()
        .ok_or("Choose a folder to index first")?;
    search_storage_index(session, &query)
}

#[cfg(windows)]
#[repr(C)]
struct ShellFileOperation {
    window: *mut std::ffi::c_void,
    function: u32,
    from: *const u16,
    to: *const u16,
    flags: u16,
    aborted: i32,
    mappings: *mut std::ffi::c_void,
    progress_title: *const u16,
}

#[cfg(windows)]
#[link(name = "Shell32")]
unsafe extern "system" {
    fn SHFileOperationW(operation: *mut ShellFileOperation) -> i32;
}

#[cfg(windows)]
fn move_paths_to_recycle_bin(paths: &[PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    const DELETE: u32 = 3;
    const FLAGS: u16 = 0x0040 | 0x0010 | 0x0004 | 0x0400;
    let mut encoded = Vec::<u16>::new();
    for path in paths {
        let shell_path = shell_storage_path(path);
        encoded.extend(shell_path.as_os_str().encode_wide());
        encoded.push(0);
    }
    encoded.push(0);
    let mut operation = ShellFileOperation {
        window: std::ptr::null_mut(),
        function: DELETE,
        from: encoded.as_ptr(),
        to: std::ptr::null(),
        flags: FLAGS,
        aborted: 0,
        mappings: std::ptr::null_mut(),
        progress_title: std::ptr::null(),
    };
    // SAFETY: the source list is a double-null-terminated array of absolute
    // UTF-16 paths and every pointer remains valid for the synchronous call.
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result != 0 {
        return Err(format!(
            "Windows could not move the selection to the Recycle Bin (code {result})"
        ));
    }
    if operation.aborted != 0 {
        return Err("Recycle Bin operation was cancelled".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn move_paths_to_recycle_bin(_paths: &[PathBuf]) -> Result<(), String> {
    Err("Storage recycling is available only on Windows".into())
}

#[tauri::command]
async fn recycle_storage_items(
    ids: Vec<String>,
    state: tauri::State<'_, StorageAnalyzerState>,
) -> Result<StorageRecycleResult, String> {
    if ids.is_empty() || ids.len() > 100 {
        return Err("Select between 1 and 100 scanned items".into());
    }
    let (generation, root, current_relative, requested, control) = {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Storage scan state is unavailable")?;
        let session = guard
            .session
            .as_ref()
            .ok_or("Choose a folder to analyze first")?;
        let mut requested = Vec::new();
        let mut unique = HashSet::new();
        for id in ids {
            if !unique.insert(id.clone()) {
                continue;
            }
            requested.push(
                session
                    .targets
                    .get(&id)
                    .cloned()
                    .ok_or("A selected item is no longer in the active scan")?,
            );
        }
        let root = session.root.clone();
        let current_relative = session
            .current
            .strip_prefix(&root)
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let (generation, control) = begin_storage_scan(&mut guard);
        (generation, root, current_relative, requested, control)
    };

    let canonical_root =
        fs::canonicalize(&root).map_err(|e| format!("Could not verify scan root: {e}"))?;
    let mut verified = Vec::new();
    for target in requested {
        let canonical = fs::canonicalize(&target.path).map_err(|_| {
            format!(
                "Selected item is no longer available: {}",
                target.path.display()
            )
        })?;
        if canonical == canonical_root || !canonical.starts_with(&canonical_root) {
            return Err("Refusing to recycle an item outside the selected scan root".into());
        }
        verified.push(StorageTarget {
            path: canonical,
            bytes: target.bytes,
            node_index: target.node_index,
        });
    }
    verified.sort_by(|left, right| {
        left.path
            .components()
            .count()
            .cmp(&right.path.components().count())
    });
    let mut roots = Vec::<StorageTarget>::new();
    for target in verified {
        if roots
            .iter()
            .any(|parent| target.path.starts_with(&parent.path))
        {
            continue;
        }
        roots.push(target);
    }
    let bytes_recycled = roots
        .iter()
        .fold(0u64, |total, target| total.saturating_add(target.bytes));
    let paths: Vec<PathBuf> = roots.iter().map(|target| target.path.clone()).collect();
    let scan_root = root.clone();
    let index = tauri::async_runtime::spawn_blocking(move || {
        move_paths_to_recycle_bin(&paths)?;
        build_storage_index(&scan_root, &control)
    })
    .await
    .map_err(|error| format!("Recycle worker stopped: {error}"))??;
    let current_node = if current_relative.as_os_str().is_empty() {
        None
    } else {
        find_storage_index_node(&index, &current_relative)
    };
    let scan = store_storage_index(&state, generation, root, current_node, index)?;
    Ok(StorageRecycleResult {
        items_recycled: roots.len() as u64,
        bytes_recycled,
        scan,
    })
}

// ---- Steam -----------------------------------------------------------------

fn validated_executable_path(path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() || path.contains('\0') {
        return Err("Application path is empty or invalid".into());
    }
    let candidate = PathBuf::from(path);
    if !candidate.is_absolute() {
        return Err("Application path must be absolute".into());
    }
    let canonical = fs::canonicalize(&candidate)
        .map_err(|_| format!("Application was not found: {}", candidate.display()))?;
    if !canonical.is_file() {
        return Err("Selected application is not a file".into());
    }
    let is_exe = canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    if !is_exe {
        return Err("Only Windows .exe applications can be launched".into());
    }
    Ok(canonical)
}

#[tauri::command]
fn pick_launch_applications() -> Result<Vec<String>, String> {
    let output = ps(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $dialog=New-Object System.Windows.Forms.OpenFileDialog; \
         $dialog.Title='Choose applications for Gaming Mode'; \
         $dialog.Filter='Applications (*.exe)|*.exe'; \
         $dialog.Multiselect=$true; $dialog.CheckFileExists=$true; \
         if($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK){ $dialog.FileNames }",
    )?;
    let mut paths = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let canonical = validated_executable_path(line)?;
        let rendered = canonical.to_string_lossy().to_string();
        push_unique(&mut paths, PathBuf::from(rendered));
    }
    Ok(paths
        .into_iter()
        .map(|path| {
            path.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_string()
        })
        .collect())
}

#[tauri::command]
fn launch_application(path: String) -> Result<LaunchApplicationResult, String> {
    let canonical = validated_executable_path(&path)?;
    let rendered = canonical
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    let escaped = rendered.replace('\'', "''");
    let status = ps(&format!(
        "$path='{escaped}'; \
         $running=Get-Process -ErrorAction SilentlyContinue | Where-Object {{ try {{ $_.Path -ieq $path }} catch {{ $false }} }} | Select-Object -First 1; \
         if($running){{ 'READY' }} else {{ Start-Process -FilePath $path -ErrorAction Stop | Out-Null; 'STARTED' }}"
    ))?;
    Ok(LaunchApplicationResult {
        started: status.lines().any(|line| line.trim() == "STARTED"),
    })
}

/// Auto-detects Steam and starts it normally when needed. PicoBoost deliberately
/// does not raise the launcher's priority above the game process.
#[tauri::command]
fn launch_steam() -> Result<String, String> {
    ps(
        "$path=$null; \
         $hkcu=Get-ItemProperty -Path 'HKCU:\\Software\\Valve\\Steam' -Name 'SteamExe' -ErrorAction SilentlyContinue; \
         if($hkcu -and $hkcu.SteamExe -and (Test-Path $hkcu.SteamExe)){ $path=$hkcu.SteamExe }; \
         if(-not $path){ $hklm=Get-ItemProperty -Path 'HKLM:\\SOFTWARE\\WOW6432Node\\Valve\\Steam' -Name 'InstallPath' -ErrorAction SilentlyContinue; \
           if($hklm -and $hklm.InstallPath){ $c=Join-Path $hklm.InstallPath 'steam.exe'; if(Test-Path $c){ $path=$c } } }; \
         if(-not $path){ foreach($c in @('C:\\Program Files (x86)\\Steam\\steam.exe','A:\\Steam\\steam.exe','D:\\Steam\\steam.exe')){ if(Test-Path $c){ $path=$c; break } } }; \
         if($path){ if(-not (Get-Process -Name 'steam' -ErrorAction SilentlyContinue)){ Start-Process -FilePath $path -ErrorAction Stop | Out-Null }; $path } else { '' }",
    )
}

// ---- Window controls (frameless titlebar) ----------------------------------

#[tauri::command]
fn window_minimize(window: tauri::Window) {
    let _ = window.minimize();
}

#[tauri::command]
fn window_toggle_maximize(window: tauri::Window) {
    if let Ok(is_max) = window.is_maximized() {
        if is_max {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
    }
}

#[tauri::command]
fn window_close(window: tauri::Window) -> Result<(), String> {
    window
        .destroy()
        .map_err(|error| format!("Could not close PicoBoost: {error}"))
}

#[tauri::command]
fn show_window(window: tauri::Window) {
    let _ = window.show();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(StorageAnalyzerState::default())
        .setup(|app| {
            // Explicitly assign the bundled icon to the native window. Windows can
            // otherwise retain the development executable icon for an unpinned
            // taskbar button even after the packaged executable has been updated.
            if let (Some(window), Some(icon)) = (
                app.get_webview_window("main"),
                app.default_window_icon().cloned(),
            ) {
                window.set_icon(icon)?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_system_info,
            get_system_details,
            get_ram,
            get_display_brightness,
            set_display_brightness,
            get_memory_snapshot,
            close_memory_apps,
            close_memory_apps_elevated,
            force_close_memory_apps,
            apply_memory_balance,
            restore_memory_balance,
            optimize_power_plan,
            restore_power_plan,
            apply_windows_gaming_settings,
            restore_windows_gaming_settings,
            start_services,
            scan_cleanup,
            run_cleanup,
            launch_steam,
            pick_launch_applications,
            launch_application,
            scan_storage_folder,
            scan_storage_folder_fast,
            storage_fast_mode_support,
            cancel_storage_scan,
            get_storage_scan_progress,
            search_storage,
            rescan_storage,
            browse_storage_item,
            storage_go_up,
            recycle_storage_items,
            show_window,
            window_minimize,
            window_toggle_maximize,
            window_close
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn cleanup_walker_only_removes_contents_of_controlled_directory() {
        let unique = format!(
            "PicoBoost-Cleanup-Test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("test directory should be created");
        fs::write(root.join("one.cache"), b"1234").expect("test file should be created");
        fs::write(nested.join("two.cache"), b"123456").expect("nested test file should be created");

        let mut scanned = CleanupStats::default();
        scan_path(&root, None, &mut scanned);
        assert_eq!(scanned.files, 2);
        assert_eq!(scanned.bytes, 10);

        let mut too_recent = CleanupStats::default();
        scan_path(&root, Some(ONE_DAY), &mut too_recent);
        assert_eq!(too_recent.files, 0);

        let mut removed = CleanupStats::default();
        purge_path(&root, None, false, &mut removed);
        assert_eq!(removed.files, 2);
        assert_eq!(removed.bytes, 10);
        assert!(root.exists(), "the approved category root must remain");
        assert_eq!(
            fs::read_dir(&root)
                .expect("root should be readable")
                .count(),
            0
        );
        fs::remove_dir(&root).expect("controlled test directory should be removable");
    }

    #[test]
    fn cleanup_command_rejects_unknown_categories_before_deleting() {
        let error = run_cleanup(vec!["arbitrary_path".into()])
            .expect_err("unknown cleanup IDs must be rejected");
        assert!(error.contains("Unknown cleanup category"));
    }

    #[test]
    fn cleanup_category_ids_are_unique() {
        let specs = cleanup_specs();
        let unique: HashSet<&str> = specs.iter().map(|spec| spec.id).collect();
        assert_eq!(unique.len(), specs.len());
    }

    #[test]
    fn power_plan_guids_require_the_canonical_shape() {
        assert!(valid_guid("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c"));
        assert!(valid_guid("CE7F7CF4-35CA-4C0D-BF54-85B9A6E822D6"));
        assert!(!valid_guid("8c5e7fdae8bf-4a96-9a85-a6e23a8c635c-"));
        assert!(!valid_guid("'; Remove-Item C:\\bad; #----------"));
    }

    #[test]
    fn brightness_percentages_respect_monitor_ranges() {
        assert_eq!(brightness_percent(20, 60, 100), 50);
        assert_eq!(brightness_percent(20, 0, 100), 0);
        assert_eq!(brightness_percent(20, 150, 100), 100);
        assert_eq!(brightness_value(20, 100, 0), 20);
        assert_eq!(brightness_value(20, 100, 50), 60);
        assert_eq!(brightness_value(20, 100, 100), 100);
        assert_eq!(brightness_value(20, 100, 150), 100);
    }

    #[test]
    fn reads_display_brightness_capability_without_changing_it() {
        let info = get_display_brightness_impl()
            .expect("connected display capability query should complete");
        assert!(info.brightness_percent <= 100);
        assert!(info.supported_monitors <= info.total_monitors);
    }

    #[test]
    fn custom_launcher_accepts_only_existing_absolute_executables() {
        let unique = format!(
            "PicoBoost-Launch-Test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("test directory should be created");
        let executable = root.join("safe application.exe");
        let text_file = root.join("not an application.txt");
        fs::write(&executable, b"test").expect("test executable should be created");
        fs::write(&text_file, b"test").expect("test text file should be created");

        assert!(validated_executable_path(executable.to_string_lossy().as_ref()).is_ok());
        assert!(validated_executable_path(text_file.to_string_lossy().as_ref()).is_err());
        assert!(validated_executable_path("relative.exe").is_err());
        assert!(
            validated_executable_path(root.join("missing.exe").to_string_lossy().as_ref()).is_err()
        );

        fs::remove_file(executable).expect("controlled executable should be removable");
        fs::remove_file(text_file).expect("controlled text file should be removable");
        fs::remove_dir(root).expect("controlled test directory should be removable");
    }

    #[test]
    fn storage_scan_measures_controlled_tree_without_exposing_the_root() {
        let unique = format!(
            "PicoBoost-Storage-Test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let nested = root.join("large folder");
        fs::create_dir_all(&nested).expect("controlled storage tree should be created");
        let root_file = root.join("small.bin");
        let nested_file = nested.join("large.bin");
        fs::write(&root_file, b"1234").expect("controlled root file should be created");
        fs::write(&nested_file, b"1234567890").expect("controlled nested file should be created");

        let canonical_root = fs::canonicalize(&root).expect("controlled root should canonicalize");
        let control = StorageScanControl::default();
        let index = build_storage_index(&canonical_root, &control)
            .expect("controlled storage index should succeed");
        assert_eq!(index.nodes.len(), 3);
        let (scan, targets) = render_storage_index_view(&canonical_root, &index, None)
            .expect("controlled storage view should render");
        assert_eq!(scan.total_bytes, 14);
        assert_eq!(scan.files, 2);
        assert_eq!(scan.folders, 1);
        assert_eq!(scan.children.len(), 2);
        assert_eq!(scan.largest_files.first().map(|item| item.bytes), Some(10));
        assert!(control.visited.load(Ordering::Relaxed) >= 4);
        assert!(control.workers.load(Ordering::Relaxed) >= 2);
        assert!(targets.values().all(|target| target.path != canonical_root));
        let mut session = StorageSession {
            root: canonical_root.clone(),
            current: canonical_root.clone(),
            current_node: None,
            targets,
            index,
        };
        let search = search_storage_index(&mut session, "large.bin")
            .expect("complete native index should be searchable");
        assert_eq!(search.total_matches, 1);
        assert_eq!(search.indexed_items, 3);
        assert_eq!(search.items.first().map(|item| item.bytes), Some(10));

        fs::remove_file(nested_file).expect("controlled nested file should be removable");
        fs::remove_file(root_file).expect("controlled root file should be removable");
        fs::remove_dir(nested).expect("controlled nested folder should be removable");
        fs::remove_dir(root).expect("controlled storage root should be removable");
    }

    #[test]
    fn storage_scan_control_cancels_active_and_superseded_walks() {
        let mut inner = StorageAnalyzerInner::default();
        let (_, first) = begin_storage_scan(&mut inner);
        let (_, second) = begin_storage_scan(&mut inner);
        assert!(first.cancelled.load(Ordering::Acquire));
        assert!(!second.cancelled.load(Ordering::Acquire));

        second.cancelled.store(true, Ordering::Release);
        let result = build_storage_index(Path::new("."), &second);
        assert!(matches!(result, Err(error) if error == "Storage scan cancelled"));
    }

    #[cfg(windows)]
    #[test]
    fn raw_volume_helpers_keep_unaligned_reads_and_paths_correct() {
        let source: Vec<u8> = (0..8192).map(|index| (index % 251) as u8).collect();
        let mut reader = SectorReader::new(std::io::Cursor::new(source.clone()), 512)
            .expect("test sector reader should initialize");
        reader
            .seek(SeekFrom::Start(509))
            .expect("unaligned test seek should succeed");
        let mut output = [0u8; 12];
        reader
            .read_exact(&mut output)
            .expect("cross-sector test read should succeed");
        assert_eq!(&output, &source[509..521]);

        let (volume, components) = ntfs_volume_path(Path::new(r"C:\Games\Example"))
            .expect("local drive path should map to a raw NTFS volume");
        assert_eq!(volume, r"\\.\C:");
        assert_eq!(components, ["Games", "Example"]);
        assert!(ntfs_volume_path(Path::new(r"\\server\share")).is_err());
    }

    #[test]
    fn reads_focused_system_details() {
        let details = read_system_details().expect("system details query should succeed");
        assert!(!details.cpu.name.trim().is_empty());
        assert!(details.cpu.physical_cores > 0);
        assert!(details.cpu.logical_processors >= details.cpu.physical_cores);
        assert!(details.cpu.load_percent.is_some_and(|load| load <= 100));
        assert!(details.cpu.current_clock_mhz.is_some_and(|clock| clock > 0));
        if let Some(temperature) = details.cpu.temperature_c {
            assert!(temperature > 0.0 && temperature < 160.0);
        }
        assert!(details.memory.total_mb > 0);
        assert!(!details.memory.memory_type.trim().is_empty());
    }

    #[test]
    fn memory_snapshot_reports_capacity_without_offering_the_windows_shell() {
        let snapshot = read_memory_snapshot().expect("memory snapshot should succeed");
        assert!(snapshot.total_mb > 0);
        assert!(snapshot.available_mb <= snapshot.total_mb);
        assert!(snapshot.used_percent <= 100);
        assert!(snapshot.commit_used_mb <= snapshot.commit_limit_mb);
        assert!(snapshot.processes.iter().all(|process| {
            !matches!(
                process.name.to_ascii_lowercase().as_str(),
                "explorer" | "picoboost" | "shellexperiencehost" | "startmenuexperiencehost"
            )
        }));
    }

    #[test]
    fn memory_priority_change_round_trips_on_the_owned_test_process() {
        let pid = std::process::id();
        let original = set_process_memory_priority(pid, 4)
            .expect("the current process memory priority should be adjustable");
        assert!((1..=5).contains(&original));
        set_process_memory_priority(pid, original)
            .expect("the current process memory priority should restore exactly");
    }

    #[cfg(windows)]
    #[test]
    fn memory_close_validation_refuses_protected_and_duplicate_targets() {
        let protected = MemoryProcess {
            pid: std::process::id(),
            name: "picoboost".into(),
            title: "PicoBoost test process".into(),
            working_set_mb: 1,
            private_mb: 1,
        };
        let (valid, results) = validate_memory_close_targets(vec![protected.clone(), protected])
            .expect("protected close targets should be reported without being opened");
        assert!(valid.is_empty());
        assert_eq!(
            results.len(),
            1,
            "duplicate PIDs must collapse to one result"
        );
        assert!(!results[0].closed);
        assert!(!results[0].can_force);
        assert!(memory_process_is_running(std::process::id()));
        let encoded = encode_memory_process_name("WizTree64");
        assert_eq!(
            decode_memory_process_name(&encoded).as_deref(),
            Some("WizTree64")
        );
        assert!(decode_memory_process_name("not-hex").is_none());
    }

    #[test]
    #[ignore = "temporarily changes and immediately restores per-user gaming settings"]
    fn reversible_gaming_settings_round_trip() {
        let state =
            apply_windows_gaming_settings(true, true).expect("gaming settings should apply");
        let verification = ps(
            "$game=(Get-ItemProperty -LiteralPath 'HKCU:\\Software\\Microsoft\\GameBar' -Name 'AutoGameModeEnabled' -ErrorAction Stop).AutoGameModeEnabled; \
             $recording=(Get-ItemProperty -LiteralPath 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\GameDVR' -Name 'VKMSaveHistoricalVideo' -ErrorAction Stop).VKMSaveHistoricalVideo; \
             \"$game|$recording\"",
        );
        let restore = restore_windows_gaming_settings(state);
        restore.expect("gaming settings should restore even after verification");
        assert_eq!(
            verification.expect("applied values should be readable"),
            "1|0"
        );
    }

    #[test]
    #[ignore = "temporarily changes and immediately restores the active power plan"]
    fn reversible_power_plan_round_trip() {
        let state = optimize_power_plan().expect("performance power plan should apply");
        let expected = state
            .created_guid
            .clone()
            .unwrap_or_else(|| "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into());
        let verification = ps("powercfg /getactivescheme 2>&1");
        let restore = restore_power_plan(state);
        restore.expect("original power plan should restore after verification");
        assert!(verification
            .expect("active power plan should be readable")
            .to_ascii_lowercase()
            .contains(&expected));
    }
}
