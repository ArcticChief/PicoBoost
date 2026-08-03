import { api } from './api';
import { confirmDialog } from './dialogs';
import { MemoryProcess, MemorySnapshot } from './types';

type Notify = (message: string) => void;
type BalanceChange = (applications: string[]) => void;
const MEMORY_BALANCE_KEY = 'picoboost_memory_balance_apps_v1';

export function loadMemoryBalanceApps(): string[] {
  try {
    const value = JSON.parse(localStorage.getItem(MEMORY_BALANCE_KEY) ?? '[]') as unknown;
    if (!Array.isArray(value)) return [];
    return [...new Set(value
      .filter((name): name is string => typeof name === 'string')
      .map(normalizeAppName)
      .filter(Boolean))].slice(0, 12);
  } catch {
    return [];
  }
}

/** Pressure-aware memory helper. It never empties global working sets or cache. */
export class MemoryToolsModal {
  private readonly overlay = document.getElementById('memory-tools-overlay') as HTMLElement;
  private readonly closeButton = document.getElementById('memory-tools-close') as HTMLButtonElement;
  private readonly refreshButton = document.getElementById('memory-tools-refresh') as HTMLButtonElement;
  private readonly closeSelectedButton = document.getElementById('memory-close-selected') as HTMLButtonElement;
  private readonly loading = document.getElementById('memory-tools-loading') as HTMLElement;
  private readonly content = document.getElementById('memory-tools-content') as HTMLElement;
  private readonly list = document.getElementById('memory-apps-list') as HTMLElement;
  private readonly selection = document.getElementById('memory-selection') as HTMLElement;
  private readonly balanceSummary = document.getElementById('memory-balance-summary') as HTMLElement;
  private readonly balanceChips = document.getElementById('memory-balance-chips') as HTMLElement;
  private snapshot: MemorySnapshot | null = null;
  private selected = new Set<number>();
  private forceCandidates: MemoryProcess[] = [];
  private elevationRequired = false;
  private elevatedGracefulAttempted = false;
  private balanceApps = new Set(loadMemoryBalanceApps());
  private busy = false;
  private previousFocus: HTMLElement | null = null;

  constructor(
    private readonly notify: Notify,
    private readonly onBalanceChange: BalanceChange = () => undefined,
  ) {
    document.getElementById('memory-tools-btn')?.addEventListener('click', () => this.open());
    document.getElementById('memory-balance-configure')?.addEventListener('click', () => this.open());
    this.closeButton.addEventListener('click', () => this.close());
    this.refreshButton.addEventListener('click', () => void this.load());
    this.closeSelectedButton.addEventListener('click', () => void this.onCloseAction());
    this.overlay.addEventListener('mousedown', (event) => {
      if (event.target === this.overlay) this.close();
    });
    document.addEventListener('keydown', (event) => {
      if (event.key === 'Escape' && !this.overlay.classList.contains('hidden') && !document.querySelector('.confirm-overlay')) {
        event.preventDefault();
        this.close();
      }
    });
  }

  private open(): void {
    if (!this.overlay.classList.contains('hidden')) return;
    this.previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.overlay.classList.remove('hidden');
    this.overlay.setAttribute('aria-hidden', 'false');
    this.closeButton.focus();
    void this.load();
  }

  private close(): void {
    if (this.busy) return;
    this.overlay.classList.add('hidden');
    this.overlay.setAttribute('aria-hidden', 'true');
    this.previousFocus?.focus();
  }

  private async load(): Promise<void> {
    if (this.busy) return;
    this.clearForceFallback();
    this.setBusy(true, 'Reading memory pressure…');
    try {
      this.snapshot = await api.getMemorySnapshot();
      this.selected.clear();
      this.render();
    } catch (error) {
      this.notify(`Memory scan stopped: ${String(error)}`);
    } finally {
      this.setBusy(false);
    }
  }

  private render(): void {
    if (!this.snapshot) return;
    const snapshot = this.snapshot;
    setText('memory-available', formatMemory(snapshot.available_mb));
    setText('memory-total', `of ${formatMemory(snapshot.total_mb)} physical RAM`);
    setText('memory-used', `${snapshot.used_percent}%`);
    setText('memory-commit', `${formatMemory(snapshot.commit_used_mb)} / ${formatMemory(snapshot.commit_limit_mb)}`);
    setText('memory-pressure', snapshot.pressure);
    setText('memory-guidance', pressureGuidance(snapshot));
    const pressureCard = document.getElementById('memory-pressure-card') as HTMLElement;
    pressureCard.dataset.pressure = snapshot.pressure.toLocaleLowerCase();

    this.list.replaceChildren();
    if (!snapshot.processes.length) {
      this.list.appendChild(element('div', 'memory-apps-empty', 'No closable visible applications were found. Windows-managed memory is already handling the rest.'));
    } else {
      snapshot.processes.forEach((process) => this.list.appendChild(this.processRow(process)));
    }
    this.updateSelection();
  }

  private processRow(process: MemoryProcess): HTMLElement {
    const row = element('div', 'memory-app-row');
    const selector = element('label', 'memory-app-selector');
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.checked = this.selected.has(process.pid);
    checkbox.setAttribute('aria-label', `Select ${process.name}`);
    checkbox.addEventListener('change', () => {
      this.clearForceFallback();
      if (checkbox.checked) this.selected.add(process.pid);
      else this.selected.delete(process.pid);
      row.classList.toggle('selected', checkbox.checked);
      this.updateSelection();
    });
    const check = element('span', 'memory-app-check');
    selector.append(checkbox, check);
    const identity = element('span', 'memory-app-identity');
    identity.append(
      element('strong', '', process.title.trim() || process.name),
      element('small', '', `${process.name}.exe · PID ${process.pid}`),
    );
    const usage = element('span', 'memory-app-usage');
    usage.append(
      element('strong', '', formatMemory(process.private_mb)),
      element('small', '', `${formatMemory(process.working_set_mb)} resident`),
    );
    const appName = normalizeAppName(process.name);
    const balance = document.createElement('button');
    balance.type = 'button';
    balance.className = 'memory-balance-toggle';
    const updateBalance = (): void => {
      const active = this.balanceApps.has(appName);
      balance.classList.toggle('active', active);
      balance.textContent = active ? 'Balanced' : 'Balance';
      balance.title = active
        ? `${process.name} will return to normal memory priority when the session ends`
        : `Favor game memory before ${process.name} while keeping it open`;
      row.classList.toggle('balanced', active);
    };
    balance.addEventListener('click', () => {
      if (this.balanceApps.has(appName)) this.balanceApps.delete(appName);
      else if (this.balanceApps.size < 12) this.balanceApps.add(appName);
      else {
        this.notify('Memory Balance supports up to 12 applications');
        return;
      }
      this.saveBalanceApps();
      updateBalance();
      this.renderBalancePlan();
    });
    updateBalance();
    row.append(selector, identity, usage, balance);
    return row;
  }

  private updateSelection(): void {
    if (this.forceCandidates.length) {
      const privateMb = this.forceCandidates.reduce((total, process) => total + process.private_mb, 0);
      this.selection.textContent = `${this.forceCandidates.length} still open · ${formatMemory(privateMb)} may be recovered`;
      const needsAdminRetry = this.elevationRequired && !this.elevatedGracefulAttempted;
      this.closeSelectedButton.textContent = needsAdminRetry
        ? 'Retry close as administrator'
        : `Force close ${this.forceCandidates.length === 1 ? 'remaining app' : `${this.forceCandidates.length} remaining apps`}`;
      this.closeSelectedButton.classList.toggle('force', !needsAdminRetry);
      this.closeSelectedButton.classList.toggle('elevated', needsAdminRetry);
      this.closeSelectedButton.disabled = this.busy;
      this.renderBalancePlan();
      return;
    }
    const selected = this.snapshot?.processes.filter((process) => this.selected.has(process.pid)) ?? [];
    const privateMb = selected.reduce((total, process) => total + process.private_mb, 0);
    this.selection.textContent = selected.length
      ? `${selected.length} selected · ${formatMemory(privateMb)} private memory`
      : 'Nothing selected to close';
    this.closeSelectedButton.textContent = 'Close selected apps';
    this.closeSelectedButton.classList.remove('force');
    this.closeSelectedButton.classList.remove('elevated');
    this.closeSelectedButton.disabled = this.busy || selected.length === 0;
    this.renderBalancePlan();
  }

  private clearForceFallback(): void {
    this.forceCandidates = [];
    this.elevationRequired = false;
    this.elevatedGracefulAttempted = false;
    this.closeSelectedButton.classList.remove('force');
    this.closeSelectedButton.classList.remove('elevated');
  }

  private async onCloseAction(): Promise<void> {
    if (this.forceCandidates.length && this.elevationRequired && !this.elevatedGracefulAttempted) await this.retryCloseElevated();
    else if (this.forceCandidates.length) await this.forceCloseRemaining();
    else await this.closeSelected();
  }

  private saveBalanceApps(): void {
    const applications = [...this.balanceApps];
    localStorage.setItem(MEMORY_BALANCE_KEY, JSON.stringify(applications));
    this.onBalanceChange(applications);
  }

  private renderBalancePlan(): void {
    const names = [...this.balanceApps].sort((left, right) => left.localeCompare(right));
    this.balanceSummary.textContent = names.length
      ? `${names.length} ${names.length === 1 ? 'app' : 'apps'} will yield memory to games`
      : 'No background apps configured';
    this.balanceChips.replaceChildren(...names.map((name) => {
      const chip = document.createElement('button');
      chip.type = 'button';
      chip.className = 'memory-balance-chip';
      chip.textContent = `${name} ×`;
      chip.title = `Remove ${name} from Gaming Session Balance`;
      chip.addEventListener('click', () => {
        this.balanceApps.delete(name);
        this.saveBalanceApps();
        this.render();
      });
      return chip;
    }));
  }

  private async closeSelected(): Promise<void> {
    if (this.busy || !this.snapshot) return;
    const processes = this.snapshot.processes.filter((process) => this.selected.has(process.pid));
    if (!processes.length) return;
    const names = processes.slice(0, 3).map((process) => process.title.trim() || process.name).join(', ');
    const extra = processes.length > 3 ? ` and ${processes.length - 3} more` : '';
    const confirmed = await confirmDialog(
      `Ask ${names}${extra} to close normally? Applications may show a save prompt. PicoBoost will not force them to exit.`,
      { title: 'Close selected applications?', confirmText: 'Request Close' },
    );
    if (!confirmed) return;

    this.setBusy(true, 'Waiting for applications to respond…');
    const beforeAvailable = this.snapshot.available_mb;
    try {
      const result = await api.closeMemoryApps(processes);
      const forceable = new Set(result.results.filter((item) => item.can_force).map((item) => item.pid));
      this.forceCandidates = processes.filter((process) => forceable.has(process.pid));
      this.elevationRequired = result.results.some((item) => item.can_force && item.needs_elevation);
      this.elevatedGracefulAttempted = false;
      this.snapshot = result.snapshot;
      this.selected.clear();
      this.render();
      const gained = result.snapshot.available_mb - beforeAvailable;
      const change = `${gained >= 0 ? '+' : ''}${formatMemoryDelta(gained)} available`;
      if (result.still_open_processes === 0) {
        this.notify(`${result.closed_processes} ${result.closed_processes === 1 ? 'application' : 'applications'} closed normally · ${change}`);
      } else if (this.elevationRequired) {
        this.notify(`${result.closed_processes} closed · Windows blocked ${this.forceCandidates.length} higher-privilege ${this.forceCandidates.length === 1 ? 'application' : 'applications'} · use Retry close as administrator`);
      } else {
        this.notify(`${result.closed_processes} closed · ${result.still_open_processes} still open · review save prompts or use the separate force-close action`);
      }
    } catch (error) {
      this.notify(`Memory recovery stopped: ${String(error)}`);
    } finally {
      this.setBusy(false);
    }
  }

  private async retryCloseElevated(): Promise<void> {
    if (this.busy || !this.forceCandidates.length) return;
    const processes = [...this.forceCandidates];
    const confirmed = await confirmDialog(
      'Windows blocked the normal close request because one or more applications have higher privileges. Retry the same graceful close with administrator approval? Applications can still show save prompts.',
      { title: 'Administrator approval required', confirmText: 'Approve & Retry' },
    );
    if (!confirmed) return;

    this.setBusy(true, 'Waiting for administrator approval…');
    try {
      const result = await api.closeMemoryAppsElevated(processes, false);
      const remaining = new Set(result.results.filter((item) => item.can_force).map((item) => item.pid));
      this.forceCandidates = processes.filter((process) => remaining.has(process.pid));
      this.elevatedGracefulAttempted = true;
      this.snapshot = result.snapshot;
      this.selected.clear();
      this.render();
      this.notify(result.still_open_processes
        ? `${result.closed_processes} closed with administrator approval · ${result.still_open_processes} still open`
        : `${result.closed_processes} ${result.closed_processes === 1 ? 'application' : 'applications'} closed normally with administrator approval`);
    } catch (error) {
      this.notify(`Administrator close stopped: ${String(error)}`);
    } finally {
      this.setBusy(false);
    }
  }

  private async forceCloseRemaining(): Promise<void> {
    if (this.busy || !this.forceCandidates.length) return;
    const processes = [...this.forceCandidates];
    const names = processes.slice(0, 3).map((process) => process.title.trim() || process.name).join(', ');
    const extra = processes.length > 3 ? ` and ${processes.length - 3} more` : '';
    const confirmed = await confirmDialog(
      `Force close ${names}${extra}? Unsaved work in these applications can be lost. PicoBoost will re-check every PID and refuse protected or changed processes.`,
      { title: 'Force close remaining applications?', confirmText: 'Force Close', danger: true },
    );
    if (!confirmed) return;

    this.setBusy(true, 'Force closing confirmed applications…');
    try {
      const result = this.elevationRequired
        ? await api.closeMemoryAppsElevated(processes, true)
        : await api.forceCloseMemoryApps(processes);
      this.snapshot = result.snapshot;
      this.clearForceFallback();
      this.selected.clear();
      this.render();
      this.notify(result.still_open_processes
        ? `${result.forced_processes} force closed · ${result.still_open_processes} could not be closed`
        : `${result.forced_processes} ${result.forced_processes === 1 ? 'application' : 'applications'} force closed`);
    } catch (error) {
      this.notify(`Force close stopped: ${String(error)}`);
    } finally {
      this.setBusy(false);
    }
  }

  private setBusy(busy: boolean, message = ''): void {
    this.busy = busy;
    this.loading.classList.toggle('hidden', !busy);
    this.content.classList.toggle('hidden', busy || !this.snapshot);
    if (message) (this.loading.querySelector('strong') as HTMLElement).textContent = message;
    this.closeButton.disabled = busy;
    this.refreshButton.disabled = busy;
    this.updateSelection();
  }
}

function pressureGuidance(snapshot: MemorySnapshot): string {
  if (snapshot.pressure === 'Critical') return 'Close a large unused app before playing';
  if (snapshot.pressure === 'Tight') return 'Review large unused apps below';
  return 'Enough memory is immediately reusable';
}

function formatMemory(megabytes: number): string {
  if (megabytes >= 1024) return `${(megabytes / 1024).toFixed(megabytes >= 10_240 ? 0 : 1)} GB`;
  return `${Math.max(0, Math.round(megabytes))} MB`;
}

function formatMemoryDelta(megabytes: number): string {
  const absolute = Math.abs(megabytes);
  const formatted = absolute >= 1024 ? `${(absolute / 1024).toFixed(1)} GB` : `${Math.round(absolute)} MB`;
  return `${megabytes < 0 ? '-' : ''}${formatted}`;
}

function normalizeAppName(name: string): string {
  return name.trim().replace(/\.exe$/i, '').toLocaleLowerCase();
}

function element(tag: string, className = '', text = ''): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function setText(id: string, value: string): void {
  const node = document.getElementById(id);
  if (node) node.textContent = value;
}
