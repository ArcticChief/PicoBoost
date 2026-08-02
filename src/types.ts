// Shared interfaces. The *Info/*Result shapes mirror the `#[derive(Serialize)]`
// structs in `src-tauri/src/lib.rs` (snake_case fields cross the IPC boundary).

export interface SystemInfo {
  cpu: string;
  os: string;
  total_ram_mb: number;
  free_ram_mb: number;
}

export interface RamInfo {
  total_mb: number;
  free_mb: number;
}

export interface MemoryProcess {
  pid: number;
  name: string;
  title: string;
  working_set_mb: number;
  private_mb: number;
}

export interface MemorySnapshot {
  total_mb: number;
  available_mb: number;
  used_percent: number;
  commit_used_mb: number;
  commit_limit_mb: number;
  pressure: 'Ready' | 'Tight' | 'Critical';
  processes: MemoryProcess[];
}

export interface MemoryCloseResult {
  requested_processes: number;
  close_requests: number;
  closed_processes: number;
  still_open_processes: number;
  forced_processes: number;
  failed_processes: number;
  results: MemoryCloseProcessResult[];
  snapshot: MemorySnapshot;
}

export interface MemoryCloseProcessResult {
  pid: number;
  name: string;
  window_requests: number;
  closed: boolean;
  forced: boolean;
  can_force: boolean;
  needs_elevation: boolean;
  detail: string;
}

export interface MemoryPriorityState {
  pid: number;
  name: string;
  original_priority: number;
}

export interface MemoryBalanceResult {
  matched_apps: number;
  balanced_processes: number;
  skipped_processes: number;
  states: MemoryPriorityState[];
  snapshot: MemorySnapshot;
}

export interface MemoryRestoreResult {
  restored_processes: number;
  skipped_processes: number;
}

export interface CpuDetails {
  name: string;
  physical_cores: number;
  logical_processors: number;
  max_clock_mhz: number | null;
  temperature_c: number | null;
}

export interface GpuDetails {
  name: string;
  driver_version: string;
  vram_total_mb: number | null;
  vram_used_mb: number | null;
  utilization_percent: number | null;
  temperature_c: number | null;
}

export interface MemoryDetails {
  total_mb: number;
  available_mb: number;
  module_count: number;
  speed_mhz: number | null;
}

export interface SystemDetails {
  cpu: CpuDetails;
  gpus: GpuDetails[];
  memory: MemoryDetails;
  os_name: string;
  os_build: string;
  active_power_plan: string;
  sensor_status: string;
}

export type CleanupGroup = 'Everyday' | 'Developer' | 'Advanced';

export interface CleanupCategory {
  id: string;
  name: string;
  description: string;
  group: CleanupGroup;
  bytes: number;
  files: number;
  default_selected: boolean;
  caution: string | null;
  available: boolean;
}

export interface CleanupRunResult {
  files_removed: number;
  bytes_freed: number;
  failed_items: number;
}

/** Which optimization steps a boost should run. One flag per feature card. */
export interface BoostOptions {
  memoryReadiness: boolean;
  powerPlan: boolean;
  gameMode: boolean;
  pauseBackgroundRecording: boolean;
  launchApplications: boolean;
}

export interface LaunchApplication {
  id: string;
  name: string;
  path: string | null;
  enabled: boolean;
  builtIn: boolean;
}

export interface LaunchApplicationResult {
  started: boolean;
}

export interface StorageItem {
  id: string;
  name: string;
  relative_path: string;
  is_directory: boolean;
  bytes: number;
  files: number;
  folders: number;
  modified_ms: number | null;
}

export interface StorageScanResult {
  root: string;
  current: string;
  total_bytes: number;
  files: number;
  folders: number;
  skipped: number;
  duration_ms: number;
  indexed_items: number;
  scan_mode: string;
  children: StorageItem[];
  largest_files: StorageItem[];
}

export interface StorageFastModeSupport {
  available: boolean;
  requires_elevation: boolean;
  volume: string | null;
  reason: string;
}

export interface StorageScanProgress {
  running: boolean;
  items_checked: number;
  elapsed_ms: number;
  workers: number;
}

export interface StorageSearchResult {
  query: string;
  total_matches: number;
  indexed_items: number;
  duration_ms: number;
  items: StorageItem[];
}

export interface StorageRecycleResult {
  items_recycled: number;
  bytes_recycled: number;
  scan: StorageScanResult;
}

export type Preset = 'Performance' | 'Balanced' | 'Minimal';

export interface RegistryDwordState {
  existed: boolean;
  value: number | null;
}

export interface WindowsGamingState {
  game_mode_enabled: RegistryDwordState | null;
  historical_video_enabled: RegistryDwordState | null;
}

export interface PowerPlanState {
  original_guid: string;
  created_guid: string | null;
}

/** Restore snapshot persisted to localStorage between a boost and its undo. */
export interface RestoreState {
  sessionId: string;
  timestamp: string;
  powerPlanState?: PowerPlanState;
  windowsGamingState?: WindowsGamingState;
  memoryPriorityStates?: MemoryPriorityState[];
  /** Legacy v1 fields are retained only so older sessions can be recovered. */
  originalPowerSchemeGuid?: string;
  stoppedServices?: string[];
}

/** Accumulated results of a boost run, rendered in the dashboard. */
export interface BoostReport {
  elapsedMs: number;
  memoryChecked: boolean;
  memoryAvailableMb: number;
  memoryBalancedApps: number;
  memoryBalancedProcesses: number;
  powerPlanApplied: boolean;
  gameModeEnabled: boolean;
  backgroundRecordingPaused: boolean;
  applicationsReady: number;
  applicationsRequested: number;
}
