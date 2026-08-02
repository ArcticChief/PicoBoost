import { BoostOptions, Preset } from './types';

/**
 * Safe-by-default session profiles. Every optimization changes a documented
 * Windows setting that PicoBoost snapshots and restores. Launching applications
 * is a convenience, so presets never enable it without an explicit user choice.
 */
export const PRESETS: Record<Preset, BoostOptions> = {
  Performance: {
    memoryReadiness: true,
    powerPlan: true,
    gameMode: true,
    pauseBackgroundRecording: true,
    launchApplications: false,
  },
  Balanced: {
    memoryReadiness: true,
    powerPlan: true,
    gameMode: true,
    pauseBackgroundRecording: false,
    launchApplications: false,
  },
  Minimal: {
    memoryReadiness: false,
    powerPlan: false,
    gameMode: true,
    pauseBackgroundRecording: false,
    launchApplications: false,
  },
};

export function defaultOptions(): BoostOptions {
  return { ...PRESETS.Performance };
}
