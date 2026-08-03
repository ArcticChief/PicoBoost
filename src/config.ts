import { BoostOptions } from './types';

/**
 * PicoBoost has one performance baseline. Every enabled optimization changes a
 * documented Windows setting that PicoBoost snapshots and restores. Launching
 * applications remains opt-in because it is preparation rather than tuning.
 */
const PERFORMANCE_OPTIONS: Readonly<BoostOptions> = {
  memoryReadiness: true,
  powerPlan: true,
  gameMode: true,
  pauseBackgroundRecording: true,
  launchApplications: false,
};

export function defaultOptions(): BoostOptions {
  return { ...PERFORMANCE_OPTIONS };
}
