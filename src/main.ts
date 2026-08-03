import { api } from './api';
import { Optimizer, LogLevel } from './optimizer';
import { defaultOptions } from './config';
import { BoostOptions, BoostReport } from './types';
import { confirmDialog } from './dialogs';
import { SystemDetailsModal } from './system-details';
import { CleanupToolsModal } from './cleanup-tools';
import { LaunchAppsModal } from './launch-apps';
import { StorageMapModal } from './storage-map';
import { loadMemoryBalanceApps, MemoryToolsModal } from './memory-tools';

// Orchestrator. Owns app-wide UI state (session options and timers), wires
// DOM events, and delegates the actual work to the Optimizer controller —
// mirroring how PicoNote's `PicoNoteApp` delegates to its feature managers.
class PicoBoostApp {
  private optimizer!: Optimizer;
  private options: BoostOptions = defaultOptions();
  private toastTimer: ReturnType<typeof setTimeout> | null = null;
  private brightnessTimer: ReturnType<typeof setTimeout> | null = null;
  private brightnessQueued: number | null = null;
  private brightnessApplying = false;
  private closing = false;
  private closeQueued = false;

  private boostBtn!: HTMLButtonElement;
  private ringProgress!: SVGCircleElement;
  private consoleEl!: HTMLElement;

  // Circumference of the progress ring (r = 68) for stroke-dashoffset animation.
  private static readonly RING_LEN = 2 * Math.PI * 68;

  constructor() {
    this.optimizer = new Optimizer({
      onLog: (msg, level) => this.log(msg, level),
      onProgress: (f) => this.setProgress(f),
    });
    void this.init().catch((error) => {
      console.error('PicoBoost initialization failed', error);
      this.toast(`Initialization problem: ${String(error)}`);
      void api.showWindow();
    });
  }

  private async init(): Promise<void> {
    this.boostBtn = document.getElementById('boost-btn') as HTMLButtonElement;
    this.ringProgress = document.getElementById('ring-progress') as unknown as SVGCircleElement;
    this.consoleEl = document.getElementById('console') as HTMLElement;

    this.ringProgress.style.strokeDasharray = String(PicoBoostApp.RING_LEN);
    this.setProgress(0);

    this.setupTitlebar();
    this.setupToggles();
    this.updateSessionSummary();
    this.applyOptionsToUI();
    void this.setupDisplayBrightness();
    const systemDetails = new SystemDetailsModal((message) => this.toast(message));
    new CleanupToolsModal((message) => this.toast(message));
    new MemoryToolsModal(
      (message) => this.toast(message),
      (applications) => this.updateMemoryBalanceSummary(applications),
    );
    this.updateMemoryBalanceSummary(loadMemoryBalanceApps());
    new StorageMapModal((message) => this.toast(message));
    new LaunchAppsModal(
      (message) => this.toast(message),
      (applications) => {
        const enabled = applications.filter((application) => application.enabled);
        const summary = document.getElementById('launch-apps-summary') as HTMLElement;
        const badge = document.getElementById('launch-apps-count') as HTMLElement;
        if (enabled.length === 0) summary.textContent = 'No applications enabled';
        else if (enabled.length === 1) summary.textContent = enabled[0].name;
        else summary.textContent = `${enabled[0].name} + ${enabled.length - 1} more`;
        badge.textContent = String(enabled.length);
      },
    );

    this.boostBtn.addEventListener('click', () => void this.onPrimaryAction());
    document.getElementById('clear-log')?.addEventListener('click', () => {
      this.consoleEl.replaceChildren();
    });

    // Route titlebar close, Alt+F4, and taskbar close through the same restore
    // guard. The guarded command ultimately destroys the window directly.
    await api.onCloseRequested((event) => {
      event.preventDefault();
      void this.onClose();
    });

    this.refreshSessionState();
    await this.loadSystemInfo();
    this.startRamPolling();

    await api.showWindow();
    // Warm the detailed snapshot after the main screen is interactive. The
    // modal then opens from cache instead of starting visible hardware work.
    setTimeout(() => void systemDetails.preload(), 700);
    this.log(
      this.optimizer.hasRestoreState()
        ? 'A PicoBoost session is active. Press RESTORE when you finish playing.'
        : 'PicoBoost ready. Review Session Tuning and press ACTIVATE.',
      'info',
    );
  }

  // ---- Titlebar -----------------------------------------------------------

  private setupTitlebar(): void {
    const minimize = document.getElementById('titlebar-minimize');
    const maximize = document.getElementById('titlebar-maximize');
    const close = document.getElementById('titlebar-close');
    [minimize, maximize, close].forEach((button) => {
      button?.addEventListener('pointerdown', (event) => event.stopPropagation());
    });
    minimize?.addEventListener('click', () => api.windowMinimize());
    maximize?.addEventListener('click', () => api.windowToggleMaximize());
    close?.addEventListener('click', (event) => {
      event.stopPropagation();
      void this.onClose();
    });
  }

  // ---- Session tuning -----------------------------------------------------

  private setupToggles(): void {
    document.getElementById('toggle-list')?.querySelectorAll<HTMLInputElement>('input[data-opt]').forEach((box) => {
      box.addEventListener('change', () => {
        const key = box.dataset.opt as keyof BoostOptions;
        this.options[key] = box.checked;
        this.updateSessionSummary();
      });
    });
  }

  private applyOptionsToUI(): void {
    document.querySelectorAll<HTMLInputElement>('input[data-opt]').forEach((box) => {
      const key = box.dataset.opt as keyof BoostOptions;
      box.checked = this.options[key];
    });
  }

  private updateSessionSummary(): void {
    const enabled = Object.values(this.options).filter(Boolean).length;
    (document.getElementById('launch-mode-name') as HTMLElement).textContent = 'Performance';
    (document.getElementById('launch-mode-detail') as HTMLElement).textContent = `${enabled} tuning ${enabled === 1 ? 'action' : 'actions'} active`;
    (document.getElementById('tuning-active-count') as HTMLElement).textContent = `${enabled} ACTIVE`;
  }

  private updateMemoryBalanceSummary(applications: string[]): void {
    const count = applications.length;
    (document.getElementById('memory-balance-count') as HTMLElement).textContent = String(count);
    (document.getElementById('memory-balance-summary-main') as HTMLElement).textContent = count
      ? `${count} background ${count === 1 ? 'app' : 'apps'} configured to yield memory`
      : 'Check pressure; configure apps to yield memory';
  }

  // ---- System stats -------------------------------------------------------

  private async loadSystemInfo(): Promise<void> {
    try {
      const info = await api.getSystemInfo();
      (document.getElementById('stat-cpu') as HTMLElement).textContent = info.cpu;
      (document.getElementById('stat-os') as HTMLElement).textContent = info.os;
      this.renderRam(info.total_ram_mb, info.free_ram_mb);
    } catch {
      (document.getElementById('stat-cpu') as HTMLElement).textContent = 'Unavailable';
    }
  }

  private startRamPolling(): void {
    const poll = async () => {
      try {
        const r = await api.getRam();
        this.renderRam(r.total_mb, r.free_mb);
      } catch {
        /* transient; ignore */
      }
    };
    // Native memory polling is cheap, but ten seconds is plenty for a status
    // gauge and keeps the optimizer almost idle while waiting for user input.
    setInterval(poll, 10_000);
  }

  private renderRam(totalMb: number, freeMb: number): void {
    const usedMb = Math.max(0, totalMb - freeMb);
    const usedGb = (usedMb / 1024).toFixed(1);
    const totalGb = (totalMb / 1024).toFixed(1);
    const pct = totalMb > 0 ? Math.round((usedMb / totalMb) * 100) : 0;
    (document.getElementById('stat-ram') as HTMLElement).textContent = `${usedGb} / ${totalGb} GB · ${pct}%`;
    const fill = document.getElementById('ram-gauge-fill') as HTMLElement;
    fill.style.width = `${pct}%`;
    fill.classList.toggle('hot', pct >= 85);
  }

  // ---- Display brightness ------------------------------------------------

  private async setupDisplayBrightness(): Promise<void> {
    const card = document.querySelector('.display-dimmer') as HTMLElement;
    const slider = document.getElementById('display-brightness-slider') as HTMLInputElement;
    const status = document.getElementById('display-brightness-status') as HTMLElement;

    slider.addEventListener('input', () => {
      const percent = Number(slider.value);
      this.renderBrightness(percent);
      if (this.brightnessTimer) clearTimeout(this.brightnessTimer);
      this.brightnessTimer = setTimeout(() => this.queueBrightnessApply(percent), 160);
    });
    slider.addEventListener('change', () => {
      if (this.brightnessTimer) {
        clearTimeout(this.brightnessTimer);
        this.brightnessTimer = null;
      }
      this.queueBrightnessApply(Number(slider.value));
    });

    try {
      const info = await api.getDisplayBrightness();
      slider.value = String(info.brightness_percent);
      this.renderBrightness(info.brightness_percent);
      if (info.supported_monitors > 0) {
        slider.disabled = false;
        status.textContent = monitorSupportText(info.supported_monitors, info.total_monitors);
      } else {
        card.classList.add('unsupported');
        status.textContent = info.total_monitors
          ? 'Hardware brightness control is unavailable'
          : 'No connected display was detected';
      }
    } catch {
      card.classList.add('unsupported');
      status.textContent = 'Brightness control is unavailable';
      (document.getElementById('display-brightness-value') as HTMLOutputElement).textContent = '—';
    }
  }

  private renderBrightness(percent: number): void {
    const bounded = Math.max(0, Math.min(100, Math.round(percent)));
    const slider = document.getElementById('display-brightness-slider') as HTMLInputElement;
    slider.style.setProperty('--brightness-progress', `${bounded}%`);
    slider.setAttribute('aria-valuetext', `${bounded}% brightness`);
    (document.getElementById('display-brightness-value') as HTMLOutputElement).textContent = `${bounded}%`;
  }

  private queueBrightnessApply(percent: number): void {
    this.brightnessQueued = Math.max(0, Math.min(100, Math.round(percent)));
    if (!this.brightnessApplying) void this.flushBrightnessQueue();
  }

  private async flushBrightnessQueue(): Promise<void> {
    const card = document.querySelector('.display-dimmer') as HTMLElement;
    const status = document.getElementById('display-brightness-status') as HTMLElement;
    this.brightnessApplying = true;
    card.classList.add('adjusting');
    try {
      while (this.brightnessQueued !== null) {
        const percent = this.brightnessQueued;
        this.brightnessQueued = null;
        status.textContent = `Adjusting supported displays to ${percent}%…`;
        try {
          const result = await api.setDisplayBrightness(percent);
          status.textContent = result.updated_monitors > 0
            ? `${result.updated_monitors} ${result.updated_monitors === 1 ? 'display' : 'displays'} set to ${result.brightness_percent}%`
            : 'No display accepted the brightness change';
        } catch (error) {
          status.textContent = 'Brightness change could not be applied';
          this.toast(`Brightness unavailable: ${String(error)}`);
        }
      }
    } finally {
      card.classList.remove('adjusting');
      this.brightnessApplying = false;
      if (this.brightnessQueued !== null) void this.flushBrightnessQueue();
    }
  }

  // ---- Boost / restore ----------------------------------------------------

  private async onPrimaryAction(): Promise<void> {
    if (this.optimizer.hasRestoreState()) await this.onRestore();
    else await this.onBoost();
  }

  private async onBoost(): Promise<void> {
    if (this.optimizer.isRunning()) return;

    const anyEnabled = Object.values(this.options).some(Boolean);
    if (!anyEnabled) {
      this.toast('Enable at least one optimization first.');
      return;
    }

    this.setBusy(true, 'BOOSTING');
    document.getElementById('dashboard')?.classList.add('hidden');
    this.log('──────── ACTIVATING GAMING MODE ────────', 'step');

    try {
      const report = await this.optimizer.boost(this.options);
      this.renderDashboard(report);
      const windowsTuned = report.powerPlanApplied || report.gameModeEnabled || report.backgroundRecordingPaused || report.memoryBalancedProcesses > 0;
      if (windowsTuned) {
        this.log(`Gaming session tuned in ${(report.elapsedMs / 1000).toFixed(2)}s.`, 'ok');
        this.log('Settings are applied. PicoBoost is not processing in the background; press RESTORE when finished.', 'info');
        this.toast('Performance session active');
      } else if (report.applicationsReady) {
        this.log('Selected applications are ready; no Windows settings were changed.', 'info');
        this.toast('Applications ready');
      } else if (report.memoryChecked) {
        this.log('Memory readiness checked; no global cache or process working sets were flushed.', 'info');
        this.toast('Memory readiness checked');
      } else {
        this.log('No selected action could be applied.', 'warn');
        this.toast('No changes applied');
      }
    } catch (e) {
      this.log(`Boost error: ${e}`, 'warn');
    } finally {
      this.setBusy(false, 'ACTIVATE');
      this.refreshSessionState();
      this.continueQueuedClose();
    }
  }

  private async onRestore(): Promise<void> {
    if (this.optimizer.isRunning()) return;
    const started = performance.now();
    this.setBusy(true, 'RESTORING');
    this.log('──────── RESTORING SETTINGS ────────', 'step');
    try {
      await this.optimizer.restore();
      document.getElementById('dashboard')?.classList.add('hidden');
      this.toast(`Restore complete in ${((performance.now() - started) / 1000).toFixed(2)}s — nothing remains active`);
    } catch (e) {
      this.log(`Restore error: ${e}`, 'warn');
    } finally {
      this.setBusy(false, 'ACTIVATE');
      this.refreshSessionState();
      this.continueQueuedClose();
    }
  }

  private async onClose(restoreWithoutPrompt = false): Promise<void> {
    if (this.closing) return;
    if (this.optimizer.isRunning()) {
      if (!this.closeQueued) {
        this.closeQueued = true;
        this.log('Close requested. PicoBoost will finish the current step, restore safely, and close.', 'info');
        this.toast('Close queued — finishing safely, then PicoBoost will exit');
      }
      return;
    }

    this.closing = true;
    try {
      if (this.optimizer.hasRestoreState()) {
        const restoreFirst = restoreWithoutPrompt || await confirmDialog(
          'A performance session is active. Restore the original Windows settings before closing?',
          { title: 'Restore before exit', confirmText: 'Restore & Close' },
        );
        if (!restoreFirst) return;

        this.setBusy(true, 'RESTORING');
        try {
          await this.optimizer.restore();
        } catch (e) {
          this.log(`Close cancelled because restore is incomplete: ${e}`, 'warn');
          this.toast('Restore incomplete — PicoBoost stayed open');
          this.setBusy(false, 'ACTIVATE');
          this.refreshSessionState();
          return;
        }
      }
      try {
        await api.windowClose();
      } catch (error) {
        this.log(`Could not close PicoBoost: ${error}`, 'warn');
        this.toast('PicoBoost could not close. Please try again.');
        this.setBusy(false, 'ACTIVATE');
        this.refreshSessionState();
      }
    } finally {
      this.closing = false;
    }
  }

  private continueQueuedClose(): void {
    if (!this.closeQueued || this.optimizer.isRunning()) return;
    this.closeQueued = false;
    void this.onClose(true);
  }

  private setBusy(busy: boolean, label: string): void {
    this.boostBtn.classList.toggle('busy', busy);
    this.boostBtn.disabled = busy;
    this.setTuningControlsDisabled(busy || this.optimizer.hasRestoreState());
    (document.getElementById('boost-label') as HTMLElement).textContent = label;
    (document.getElementById('boost-sublabel') as HTMLElement).textContent = label === 'RESTORING'
      ? 'RETURNING TO NORMAL'
      : busy ? 'APPLYING SAFELY' : 'PERFORMANCE SESSION';
    this.boostBtn.setAttribute('aria-busy', String(busy));
    this.boostBtn.setAttribute('aria-label', label === 'RESTORING'
      ? 'Restoring the original Windows settings'
      : busy ? 'Activating the performance gaming session' : 'Activate the performance gaming session');
    const launchPanel = document.getElementById('launch-panel') as HTMLElement;
    launchPanel.classList.toggle('working', busy);
    (document.getElementById('launch-panel-title') as HTMLElement).textContent = label === 'RESTORING'
      ? 'Restoring your original setup'
      : busy ? 'Preparing the gaming session' : 'Ready to optimize';
    (document.getElementById('launch-state-chip') as HTMLElement).lastChild!.textContent = label === 'RESTORING'
      ? ' RESTORING' : busy ? ' APPLYING' : ' READY';
    const status = document.getElementById('session-state') as HTMLElement;
    status.classList.toggle('working', busy);
    if (busy) {
      (document.getElementById('session-state-text') as HTMLElement).textContent = label === 'RESTORING'
        ? 'Restoring original Windows settings…'
        : 'Applying selected Windows settings…';
    }
    if (!busy) this.setProgress(0);
  }

  private refreshSessionState(): void {
    const active = this.optimizer.hasRestoreState();
    const status = document.getElementById('session-state') as HTMLElement;
    const launchPanel = document.getElementById('launch-panel') as HTMLElement;
    status.classList.remove('working');
    this.setTuningControlsDisabled(active);
    this.boostBtn.classList.toggle('active', active);
    launchPanel.classList.toggle('active', active);
    launchPanel.classList.remove('working');
    this.boostBtn.title = active ? 'Restore the original Windows state' : 'Activate the performance gaming session';
    this.boostBtn.setAttribute('aria-label', active ? 'Restore the original Windows state' : 'Activate the performance gaming session');
    this.boostBtn.setAttribute('aria-busy', 'false');
    (document.getElementById('boost-label') as HTMLElement).textContent = active ? 'RESTORE' : 'ACTIVATE';
    (document.getElementById('boost-sublabel') as HTMLElement).textContent = active ? 'END GAMING SESSION' : 'PERFORMANCE SESSION';
    (document.getElementById('launch-panel-title') as HTMLElement).textContent = active ? 'Gaming session is active' : 'Ready to optimize';
    (document.getElementById('launch-state-chip') as HTMLElement).lastChild!.textContent = active ? ' ACTIVE' : ' READY';
    status.classList.toggle('active', active);
    (document.getElementById('session-state-text') as HTMLElement).textContent = active
      ? 'Settings applied · Restore when finished'
      : 'Reversible session ready';
  }

  private setTuningControlsDisabled(disabled: boolean): void {
    document.querySelectorAll<HTMLInputElement>('input[data-opt]').forEach((input) => {
      input.disabled = disabled;
    });
    document.querySelectorAll<HTMLButtonElement>('.tuning-config-btn').forEach((button) => {
      button.disabled = disabled;
    });
  }

  // ---- Rendering helpers --------------------------------------------------

  private setProgress(fraction: number): void {
    const offset = PicoBoostApp.RING_LEN * (1 - Math.max(0, Math.min(1, fraction)));
    this.ringProgress.style.strokeDashoffset = String(offset);
  }

  private renderDashboard(r: BoostReport): void {
    const set = (id: string, v: string) => ((document.getElementById(id) as HTMLElement).textContent = v);
    set('d-time', `${(r.elapsedMs / 1000).toFixed(1)}s`);
    set('d-power', r.powerPlanApplied ? 'High' : 'Kept');
    set('d-game-mode', r.gameModeEnabled ? 'On' : 'Kept');
    set('d-recording', r.backgroundRecordingPaused ? 'Paused' : 'Kept');
    set('d-memory', r.memoryBalancedProcesses ? `${r.memoryBalancedApps}/${r.memoryBalancedProcesses}` : 'Ready');
    const launchValue = r.applicationsRequested
      ? `${r.applicationsReady}/${r.applicationsRequested}`
      : 'Kept';
    set('d-launch', launchValue);
    document.getElementById('dashboard')?.classList.remove('hidden');
  }

  private log(message: string, level: LogLevel): void {
    const line = document.createElement('div');
    line.className = `log-line ${level}`;
    const time = new Date().toLocaleTimeString([], { hour12: false });
    const timeElement = document.createElement('span');
    timeElement.className = 'log-time';
    timeElement.textContent = time;
    const messageElement = document.createElement('span');
    messageElement.className = 'log-msg';
    messageElement.textContent = message;
    line.append(timeElement, messageElement);
    this.consoleEl.appendChild(line);
    this.consoleEl.scrollTop = this.consoleEl.scrollHeight;
  }

  private toast(message: string): void {
    const el = document.getElementById('app-toast') as HTMLElement;
    (document.getElementById('app-toast-text') as HTMLElement).textContent = message;
    el.classList.remove('hidden');
    if (this.toastTimer) clearTimeout(this.toastTimer);
    this.toastTimer = setTimeout(() => el.classList.add('hidden'), 2600);
  }
}

function monitorSupportText(supported: number, total: number): string {
  if (supported === total) return `${supported} ${supported === 1 ? 'display' : 'displays'} linked`;
  return `${supported} of ${total} displays linked`;
}

document.addEventListener('DOMContentLoaded', () => new PicoBoostApp());
