import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow, CloseRequestedEvent } from '@tauri-apps/api/window';
import {
  SystemInfo,
  SystemDetails,
  RamInfo,
  MemorySnapshot,
  MemoryProcess,
  MemoryCloseResult,
  MemoryBalanceResult,
  MemoryPriorityState,
  MemoryRestoreResult,
  PowerPlanState,
  WindowsGamingState,
  CleanupCategory,
  CleanupRunResult,
  LaunchApplicationResult,
  StorageScanResult,
  StorageFastModeSupport,
  StorageScanProgress,
  StorageSearchResult,
  StorageRecycleResult,
} from './types';

// Single chokepoint for every backend call. Frontend code calls `api.*` rather
// than importing `@tauri-apps/*` directly. Rust command names are snake_case;
// params are passed as camelCase and Tauri auto-converts.
export const api = {
  onCloseRequested: async (handler: (event: CloseRequestedEvent) => void | Promise<void>): Promise<void> => {
    await getCurrentWindow().onCloseRequested(handler);
  },

  showWindow: async (): Promise<void> => {
    return await invoke<void>('show_window');
  },

  windowMinimize: async (): Promise<void> => {
    return await invoke<void>('window_minimize');
  },

  windowToggleMaximize: async (): Promise<void> => {
    return await invoke<void>('window_toggle_maximize');
  },

  windowClose: async (): Promise<void> => {
    return await invoke<void>('window_close');
  },

  getSystemInfo: async (): Promise<SystemInfo> => {
    return await invoke<SystemInfo>('get_system_info');
  },

  getSystemDetails: async (): Promise<SystemDetails> => {
    return await invoke<SystemDetails>('get_system_details');
  },

  getRam: async (): Promise<RamInfo> => {
    return await invoke<RamInfo>('get_ram');
  },

  getMemorySnapshot: async (): Promise<MemorySnapshot> => {
    return await invoke<MemorySnapshot>('get_memory_snapshot');
  },

  closeMemoryApps: async (processes: MemoryProcess[]): Promise<MemoryCloseResult> => {
    return await invoke<MemoryCloseResult>('close_memory_apps', { processes });
  },

  forceCloseMemoryApps: async (processes: MemoryProcess[]): Promise<MemoryCloseResult> => {
    return await invoke<MemoryCloseResult>('force_close_memory_apps', { processes });
  },

  closeMemoryAppsElevated: async (processes: MemoryProcess[], force: boolean): Promise<MemoryCloseResult> => {
    return await invoke<MemoryCloseResult>('close_memory_apps_elevated', { processes, force });
  },

  applyMemoryBalance: async (names: string[]): Promise<MemoryBalanceResult> => {
    return await invoke<MemoryBalanceResult>('apply_memory_balance', { names });
  },

  restoreMemoryBalance: async (states: MemoryPriorityState[]): Promise<MemoryRestoreResult> => {
    return await invoke<MemoryRestoreResult>('restore_memory_balance', { states });
  },

  scanCleanup: async (): Promise<CleanupCategory[]> => {
    return await invoke<CleanupCategory[]>('scan_cleanup');
  },

  runCleanup: async (ids: string[]): Promise<CleanupRunResult> => {
    return await invoke<CleanupRunResult>('run_cleanup', { ids });
  },

  optimizePowerPlan: async (): Promise<PowerPlanState> => {
    return await invoke<PowerPlanState>('optimize_power_plan');
  },

  restorePowerPlan: async (state: PowerPlanState): Promise<void> => {
    return await invoke<void>('restore_power_plan', { state });
  },

  startServices: async (names: string[]): Promise<string[]> => {
    return await invoke<string[]>('start_services', { names });
  },

  applyWindowsGamingSettings: async (enableGameMode: boolean, pauseBackgroundRecording: boolean): Promise<WindowsGamingState> => {
    return await invoke<WindowsGamingState>('apply_windows_gaming_settings', { enableGameMode, pauseBackgroundRecording });
  },

  restoreWindowsGamingSettings: async (state: WindowsGamingState): Promise<void> => {
    return await invoke<void>('restore_windows_gaming_settings', { state });
  },

  launchSteam: async (): Promise<string> => {
    return await invoke<string>('launch_steam');
  },

  pickLaunchApplications: async (): Promise<string[]> => {
    return await invoke<string[]>('pick_launch_applications');
  },

  launchApplication: async (path: string): Promise<LaunchApplicationResult> => {
    return await invoke<LaunchApplicationResult>('launch_application', { path });
  },

  scanStorageFolder: async (path: string): Promise<StorageScanResult> => {
    return await invoke<StorageScanResult>('scan_storage_folder', { path });
  },

  scanStorageFolderFast: async (path: string): Promise<StorageScanResult> => {
    return await invoke<StorageScanResult>('scan_storage_folder_fast', { path });
  },

  storageFastModeSupport: async (path: string): Promise<StorageFastModeSupport> => {
    return await invoke<StorageFastModeSupport>('storage_fast_mode_support', { path });
  },

  cancelStorageScan: async (): Promise<void> => {
    return await invoke<void>('cancel_storage_scan');
  },

  getStorageScanProgress: async (): Promise<StorageScanProgress> => {
    return await invoke<StorageScanProgress>('get_storage_scan_progress');
  },

  searchStorage: async (query: string): Promise<StorageSearchResult> => {
    return await invoke<StorageSearchResult>('search_storage', { query });
  },

  rescanStorage: async (): Promise<StorageScanResult> => {
    return await invoke<StorageScanResult>('rescan_storage');
  },

  browseStorageItem: async (id: string): Promise<StorageScanResult> => {
    return await invoke<StorageScanResult>('browse_storage_item', { id });
  },

  storageGoUp: async (): Promise<StorageScanResult> => {
    return await invoke<StorageScanResult>('storage_go_up');
  },

  recycleStorageItems: async (ids: string[]): Promise<StorageRecycleResult> => {
    return await invoke<StorageRecycleResult>('recycle_storage_items', { ids });
  },
};
