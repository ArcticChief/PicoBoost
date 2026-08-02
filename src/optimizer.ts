import { api } from './api';
import {
  BoostOptions,
  BoostReport,
  MemoryPriorityState,
  PowerPlanState,
  RegistryDwordState,
  RestoreState,
  WindowsGamingState,
} from './types';
import { loadLaunchApplications } from './launch-apps';
import { loadMemoryBalanceApps } from './memory-tools';

const RESTORE_KEY = 'picoboost_restore_state_v1';

export type LogLevel = 'step' | 'info' | 'ok' | 'warn';

interface OptimizerCallbacks {
  onLog: (message: string, level: LogLevel) => void;
  onProgress: (fraction: number) => void;
}

/** Runs a small set of reversible, Windows-supported session optimizations. */
export class Optimizer {
  private running = false;

  constructor(private cb: OptimizerCallbacks) {}

  isRunning(): boolean {
    return this.running;
  }

  hasRestoreState(): boolean {
    return this.loadRestoreState() !== null;
  }

  private loadRestoreState(): RestoreState | null {
    try {
      const raw = localStorage.getItem(RESTORE_KEY);
      if (!raw) return null;
      const parsed = parseRestoreState(JSON.parse(raw) as unknown);
      if (!parsed || !this.hasReversibleChange(parsed)) {
        this.clearRestoreState();
        return null;
      }
      return parsed;
    } catch {
      this.clearRestoreState();
      return null;
    }
  }

  private saveRestoreState(state: RestoreState): void {
    localStorage.setItem(RESTORE_KEY, JSON.stringify(state));
  }

  private clearRestoreState(): void {
    localStorage.removeItem(RESTORE_KEY);
  }

  private hasReversibleChange(state: RestoreState): boolean {
    return Boolean(
      state.powerPlanState ||
      state.windowsGamingState ||
      state.memoryPriorityStates?.length ||
      state.originalPowerSchemeGuid ||
      state.stoppedServices?.length,
    );
  }

  async boost(opts: BoostOptions): Promise<BoostReport> {
    this.running = true;
    const start = performance.now();
    const { onLog, onProgress } = this.cb;
    const report: BoostReport = {
      elapsedMs: 0,
      memoryChecked: false,
      memoryAvailableMb: 0,
      memoryBalancedApps: 0,
      memoryBalancedProcesses: 0,
      powerPlanApplied: false,
      gameModeEnabled: false,
      backgroundRecordingPaused: false,
      applicationsReady: 0,
      applicationsRequested: 0,
    };
    const restore: RestoreState = this.loadRestoreState() ?? {
      sessionId: crypto.randomUUID(),
      timestamp: new Date().toISOString(),
    };

    const checkpoint = (): void => {
      if (this.hasReversibleChange(restore)) this.saveRestoreState(restore);
    };

    const steps: Array<{ enabled: boolean; run: () => Promise<void> }> = [
      {
        enabled: opts.memoryReadiness,
        run: async () => {
          onLog('Memory readiness…', 'step');
          try {
            const memory = await api.getMemorySnapshot();
            const availablePercent = memory.total_mb > 0 ? (memory.available_mb / memory.total_mb) * 100 : 0;
            report.memoryChecked = true;
            report.memoryAvailableMb = memory.available_mb;
            const balanceApps = loadMemoryBalanceApps();
            if (restore.memoryPriorityStates?.length) {
              report.memoryBalancedProcesses = restore.memoryPriorityStates.length;
              report.memoryBalancedApps = new Set(restore.memoryPriorityStates.map((state) => state.name.toLocaleLowerCase())).size;
              onLog(`  Session balance already active for ${report.memoryBalancedProcesses} process(es)`, 'info');
            } else if (balanceApps.length) {
              const balanced = await api.applyMemoryBalance(balanceApps);
              report.memoryBalancedApps = balanced.matched_apps;
              report.memoryBalancedProcesses = balanced.balanced_processes;
              report.memoryAvailableMb = balanced.snapshot.available_mb;
              if (balanced.states.length) {
                restore.memoryPriorityStates = balanced.states;
                try {
                  checkpoint();
                } catch (checkpointError) {
                  try {
                    await api.restoreMemoryBalance(balanced.states);
                    restore.memoryPriorityStates = undefined;
                  } catch (rollbackError) {
                    throw new Error(`Memory checkpoint failed (${checkpointError}); immediate rollback also failed (${rollbackError})`);
                  }
                  throw new Error(`Memory checkpoint failed; priorities were restored (${checkpointError})`);
                }
                onLog(`  ${balanced.matched_apps} configured app(s), ${balanced.balanced_processes} process(es) balanced`, 'ok');
                onLog('  Windows will favor game pages first when memory pressure rises', 'info');
              } else {
                onLog('  Configured background apps are not currently running', 'info');
              }
              if (balanced.skipped_processes) {
                onLog(`  ${balanced.skipped_processes} protected or inaccessible process(es) skipped`, 'info');
              }
            } else {
              onLog('  No background apps configured for session balance', 'info');
            }
            if (availablePercent < 15 && memory.available_mb < 4_096) {
              onLog(`  Memory is tight: ${formatMemory(memory.available_mb)} immediately available`, 'warn');
              onLog('  Close unused apps from Memory Readiness for immediate recovery', 'info');
            } else {
              onLog(`  Ready: ${formatMemory(memory.available_mb)} immediately available`, 'ok');
            }
            onLog('  Useful Windows cache and working sets remain intact', 'info');
          } catch (error) {
            onLog(`  Memory pressure could not be checked (${error})`, 'warn');
          }
        },
      },
      {
        enabled: opts.powerPlan,
        run: async () => {
          onLog('Performance power plan…', 'step');
          if (restore.powerPlanState || restore.originalPowerSchemeGuid) {
            report.powerPlanApplied = true;
            onLog('  Already active; original plan remains recorded', 'info');
            return;
          }
          try {
            const appliedState = await api.optimizePowerPlan();
            restore.powerPlanState = appliedState;
            try {
              checkpoint();
            } catch (checkpointError) {
              try {
                await api.restorePowerPlan(appliedState);
                restore.powerPlanState = undefined;
              } catch (rollbackError) {
                throw new Error(`Safety checkpoint failed (${checkpointError}); immediate rollback also failed (${rollbackError})`);
              }
              throw new Error(`Safety checkpoint failed; the power-plan change was rolled back (${checkpointError})`);
            }
            report.powerPlanApplied = true;
            onLog('  High Performance enabled for this session', 'ok');
          } catch (error) {
            onLog(`  Power plan unchanged (${error})`, 'warn');
          }
        },
      },
      {
        enabled: opts.gameMode || opts.pauseBackgroundRecording,
        run: async () => {
          onLog('Windows gaming settings…', 'step');
          if (restore.windowsGamingState) {
            report.gameModeEnabled = opts.gameMode;
            report.backgroundRecordingPaused = opts.pauseBackgroundRecording;
            onLog('  Gaming settings already active and safely recorded', 'info');
            return;
          }
          try {
            const appliedState = await api.applyWindowsGamingSettings(
              opts.gameMode,
              opts.pauseBackgroundRecording,
            );
            restore.windowsGamingState = appliedState;
            try {
              checkpoint();
            } catch (checkpointError) {
              try {
                await api.restoreWindowsGamingSettings(appliedState);
                restore.windowsGamingState = undefined;
              } catch (rollbackError) {
                throw new Error(`Safety checkpoint failed (${checkpointError}); immediate rollback also failed (${rollbackError})`);
              }
              throw new Error(`Safety checkpoint failed; gaming settings were rolled back (${checkpointError})`);
            }
            report.gameModeEnabled = opts.gameMode;
            report.backgroundRecordingPaused = opts.pauseBackgroundRecording;
            if (opts.gameMode) onLog('  Windows Game Mode enabled', 'ok');
            if (opts.pauseBackgroundRecording) onLog('  Background replay recording paused', 'ok');
          } catch (error) {
            onLog(`  Windows gaming settings unchanged (${error})`, 'warn');
          }
        },
      },
      {
        enabled: opts.launchApplications,
        run: async () => {
          const applications = loadLaunchApplications().filter((application) => application.enabled);
          report.applicationsRequested = applications.length;
          onLog('Launch applications…', 'step');
          if (!applications.length) {
            onLog('  No applications are enabled — skipped', 'warn');
            return;
          }
          for (const application of applications) {
            try {
              if (application.builtIn) {
                const path = await api.launchSteam();
                if (!path) {
                  onLog('  Steam was not found — skipped', 'warn');
                  continue;
                }
                report.applicationsReady += 1;
                onLog('  Steam ready at normal priority', 'ok');
              } else if (application.path) {
                const result = await api.launchApplication(application.path);
                report.applicationsReady += 1;
                onLog(`  ${application.name} ${result.started ? 'started' : 'already running'}`, 'ok');
              }
            } catch (error) {
              onLog(`  ${application.name} skipped (${error})`, 'warn');
            }
          }
        },
      },
    ];

    try {
      const active = steps.filter((step) => step.enabled);
      let done = 0;
      onProgress(0);
      for (const step of active) {
        await step.run();
        done += 1;
        onProgress(done / active.length);
      }
      report.elapsedMs = performance.now() - start;
      onProgress(1);
      onLog('No files, services, caches, or application working sets were removed.', 'info');
      return report;
    } finally {
      this.running = false;
    }
  }

  async restore(): Promise<void> {
    this.running = true;
    const started = performance.now();
    const { onLog } = this.cb;
    const state = this.loadRestoreState();
    const failures: string[] = [];
    onLog('Restoring the pre-game Windows state…', 'step');

    try {
      if (state?.memoryPriorityStates?.length) {
        try {
          const restored = await api.restoreMemoryBalance(state.memoryPriorityStates);
          state.memoryPriorityStates = undefined;
          onLog(`  Normal memory priority restored for ${restored.restored_processes} process(es)`, 'ok');
          if (restored.skipped_processes) {
            onLog(`  ${restored.skipped_processes} app process(es) had already closed`, 'info');
          }
        } catch (error) {
          failures.push('memory priorities');
          onLog(`  Memory-priority restore failed (${error})`, 'warn');
        }
      }

      if (state?.windowsGamingState) {
        try {
          await api.restoreWindowsGamingSettings(state.windowsGamingState);
          state.windowsGamingState = undefined;
          onLog('  Game Mode and recording preferences restored', 'ok');
        } catch (error) {
          failures.push('Windows gaming settings');
          onLog(`  Gaming settings restore failed (${error})`, 'warn');
        }
      }

      const powerState: PowerPlanState | undefined = state?.powerPlanState ?? (
        state?.originalPowerSchemeGuid
          ? { original_guid: state.originalPowerSchemeGuid, created_guid: null }
          : undefined
      );
      if (state && powerState) {
        try {
          await api.restorePowerPlan(powerState);
          state.powerPlanState = undefined;
          state.originalPowerSchemeGuid = undefined;
          onLog('  Original power plan restored', 'ok');
        } catch (error) {
          failures.push('power plan');
          onLog(`  Power plan restore failed (${error})`, 'warn');
        }
      }

      // Compatibility recovery for a session made by PicoBoost v1. New
      // activations never stop services.
      if (state?.stoppedServices?.length) {
        try {
          const requested = [...state.stoppedServices];
          const started = await api.startServices(requested);
          const restored = new Set(started.map((name) => name.toLowerCase()));
          state.stoppedServices = requested.filter((name) => !restored.has(name.toLowerCase()));
          if (state.stoppedServices.length) failures.push('legacy services');
          onLog(`  Recovered ${started.length} legacy service(s)`, state.stoppedServices.length ? 'warn' : 'ok');
        } catch (error) {
          failures.push('legacy services');
          onLog(`  Legacy service recovery failed (${error})`, 'warn');
        }
      }

      if (state && this.hasReversibleChange(state)) this.saveRestoreState(state);
      else this.clearRestoreState();

      if (failures.length) {
        const unique = [...new Set(failures)];
        onLog(`Restore incomplete: ${unique.join(', ')}.`, 'warn');
        throw new Error(`Could not restore ${unique.join(', ')}`);
      }
      const elapsedSeconds = (performance.now() - started) / 1000;
      onLog(
        `Restore completed in ${elapsedSeconds.toFixed(2)}s. Gaming session ended; no background task remains.`,
        'ok',
      );
    } finally {
      this.running = false;
    }
  }
}

function parseRestoreState(value: unknown): RestoreState | null {
  if (!isRecord(value)) return null;
  const state: RestoreState = {
    sessionId: typeof value.sessionId === 'string' ? value.sessionId : 'recovered-session',
    timestamp: typeof value.timestamp === 'string' ? value.timestamp : new Date(0).toISOString(),
  };

  if (isPowerPlanState(value.powerPlanState)) state.powerPlanState = value.powerPlanState;
  if (isWindowsGamingState(value.windowsGamingState)) state.windowsGamingState = value.windowsGamingState;
  if (Array.isArray(value.memoryPriorityStates)) {
    const priorities = value.memoryPriorityStates.filter(isMemoryPriorityState);
    if (priorities.length === value.memoryPriorityStates.length && priorities.length <= 96) {
      state.memoryPriorityStates = priorities;
    }
  }
  if (typeof value.originalPowerSchemeGuid === 'string' && isGuid(value.originalPowerSchemeGuid)) {
    state.originalPowerSchemeGuid = value.originalPowerSchemeGuid;
  }
  if (Array.isArray(value.stoppedServices)) {
    state.stoppedServices = value.stoppedServices.filter((name): name is string => typeof name === 'string');
  }
  return state;
}

function isMemoryPriorityState(value: unknown): value is MemoryPriorityState {
  return isRecord(value)
    && Number.isInteger(value.pid)
    && Number(value.pid) > 0
    && typeof value.name === 'string'
    && value.name.length > 0
    && Number.isInteger(value.original_priority)
    && Number(value.original_priority) >= 1
    && Number(value.original_priority) <= 5;
}

function isPowerPlanState(value: unknown): value is PowerPlanState {
  return isRecord(value)
    && typeof value.original_guid === 'string'
    && isGuid(value.original_guid)
    && (value.created_guid === null || (typeof value.created_guid === 'string' && isGuid(value.created_guid)));
}

function isWindowsGamingState(value: unknown): value is WindowsGamingState {
  if (!isRecord(value)) return false;
  return isOptionalRegistryState(value.game_mode_enabled)
    && isOptionalRegistryState(value.historical_video_enabled);
}

function isOptionalRegistryState(value: unknown): value is RegistryDwordState | null {
  if (value === null) return true;
  if (!isRecord(value) || typeof value.existed !== 'boolean') return false;
  if (!value.existed) return value.value === null;
  return Number.isInteger(value.value) && Number(value.value) >= 0 && Number(value.value) <= 0xffff_ffff;
}

function isGuid(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function formatMemory(megabytes: number): string {
  return megabytes >= 1024 ? `${(megabytes / 1024).toFixed(1)} GB` : `${Math.round(megabytes)} MB`;
}
