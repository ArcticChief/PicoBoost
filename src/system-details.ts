import { api } from './api';
import { CpuDetails, GpuDetails, SystemDetails } from './types';

type Notify = (message: string) => void;

interface Metric {
  label: string;
  value: string;
  tone?: 'good' | 'warm' | 'hot' | 'muted';
}

/** Focused system-information modal, kept separate from the app orchestrator. */
export class SystemDetailsModal {
  private static readonly CACHE_TTL_MS = 2 * 60 * 1000;
  private readonly overlay: HTMLElement;
  private readonly content: HTMLElement;
  private readonly closeButton: HTMLButtonElement;
  private readonly trigger: HTMLElement;
  private requestId = 0;
  private cachedDetails: SystemDetails | null = null;
  private cachedAt = 0;
  private inFlight: Promise<SystemDetails> | null = null;
  private previousFocus: HTMLElement | null = null;

  constructor(private readonly notify: Notify) {
    this.overlay = document.getElementById('system-details-overlay') as HTMLElement;
    this.content = document.getElementById('system-details-content') as HTMLElement;
    this.closeButton = document.getElementById('system-details-close') as HTMLButtonElement;
    this.trigger = document.getElementById('system-card') as HTMLElement;

    this.trigger.addEventListener('click', () => void this.open());
    this.trigger.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        void this.open();
      }
    });
    this.closeButton.addEventListener('click', () => this.close());
    this.overlay.addEventListener('mousedown', (event) => {
      if (event.target === this.overlay) this.close();
    });
    document.addEventListener('keydown', (event) => {
      if (
        event.key === 'Escape' &&
        !document.querySelector('.confirm-overlay') &&
        !this.overlay.classList.contains('hidden')
      ) {
        event.preventDefault();
        this.close();
      }
    });
  }

  /** Warms the modal cache quietly; failures remain invisible until explicitly opened. */
  async preload(): Promise<void> {
    try {
      await this.loadDetails();
    } catch {
      // Hardware telemetry is best-effort. Opening the modal still offers retry UI.
    }
  }

  private async open(): Promise<void> {
    if (!this.overlay.classList.contains('hidden')) return;
    this.previousFocus = document.activeElement as HTMLElement | null;
    this.overlay.classList.remove('hidden');
    this.overlay.setAttribute('aria-hidden', 'false');
    this.closeButton.focus();

    const requestId = ++this.requestId;
    if (this.cachedDetails) {
      this.render(this.cachedDetails);
      if (Date.now() - this.cachedAt > SystemDetailsModal.CACHE_TTL_MS) {
        void this.refreshVisible(requestId);
      }
      return;
    }

    this.renderLoading();
    try {
      const details = await this.loadDetails();
      if (requestId === this.requestId) this.render(details);
    } catch (error) {
      if (requestId === this.requestId) this.renderError(String(error));
    }
  }

  private loadDetails(): Promise<SystemDetails> {
    if (this.inFlight) return this.inFlight;
    const request = api.getSystemDetails().then((details) => {
      this.cachedDetails = details;
      this.cachedAt = Date.now();
      return details;
    });
    this.inFlight = request;
    void request.finally(() => {
      if (this.inFlight === request) this.inFlight = null;
    }).catch(() => {
      // The caller owns error presentation; this branch only settles `finally`.
    });
    return request;
  }

  private async refreshVisible(requestId: number): Promise<void> {
    try {
      const details = await this.loadDetails();
      if (requestId === this.requestId && !this.overlay.classList.contains('hidden')) {
        this.render(details);
      }
    } catch {
      // Keep the usable cached snapshot instead of replacing it with an error.
    }
  }

  private close(): void {
    this.requestId += 1;
    this.overlay.classList.add('hidden');
    this.overlay.setAttribute('aria-hidden', 'true');
    this.previousFocus?.focus();
  }

  private renderLoading(): void {
    this.content.replaceChildren();
    const loading = el('div', 'system-details-loading');
    loading.append(el('span', 'details-spinner'), el('span', '', 'Reading hardware…'));
    this.content.appendChild(loading);
  }

  private renderError(message: string): void {
    this.content.replaceChildren();
    const error = el('div', 'system-details-error');
    error.append(
      el('div', 'details-error-icon', '!'),
      el('div', 'details-error-title', 'System information unavailable'),
      el('div', 'details-error-message', message),
    );
    const retry = el('button', 'details-retry', 'Try again') as HTMLButtonElement;
    retry.type = 'button';
    retry.addEventListener('click', () => {
      this.close();
      void this.open();
    });
    error.appendChild(retry);
    this.content.appendChild(error);
  }

  private render(details: SystemDetails): void {
    this.content.replaceChildren();

    const hardwareGrid = el('div', 'details-hardware-grid');
    hardwareGrid.appendChild(this.hardwareCard('CPU', details.cpu.name, this.cpuMetrics(details.cpu)));

    if (details.gpus.length) {
      details.gpus.forEach((gpu, index) => {
        const label = details.gpus.length > 1 ? `GPU ${index + 1}` : 'GPU';
        hardwareGrid.appendChild(this.hardwareCard(label, gpu.name, this.gpuMetrics(gpu)));
      });
    } else {
      hardwareGrid.appendChild(this.unavailableGpuCard());
    }
    this.content.appendChild(hardwareGrid);

    const summaryGrid = el('div', 'details-summary-grid');
    summaryGrid.append(
      this.memoryCard(details),
      this.environmentCard(details),
    );
    this.content.appendChild(summaryGrid);

    const sensorNote = el('div', 'details-sensor-note');
    sensorNote.append(el('span', 'details-sensor-pulse'), el('span', '', details.sensor_status));
    this.content.appendChild(sensorNote);
  }

  private hardwareCard(kind: string, name: string, metrics: Metric[]): HTMLElement {
    const card = el('article', 'details-hardware-card');
    const head = el('div', 'details-hardware-head');
    head.appendChild(el('span', `details-kind details-kind-${kind.startsWith('GPU') ? 'gpu' : 'cpu'}`, kind));

    const copyButton = el('button', 'details-copy-name') as HTMLButtonElement;
    copyButton.type = 'button';
    copyButton.title = `Copy ${kind} name`;
    copyButton.setAttribute('aria-label', `Copy ${kind} name: ${name}`);
    copyButton.append(
      el('span', 'details-device-name', name),
      copyIcon(),
    );
    copyButton.addEventListener('click', () => void this.copyName(name, kind, copyButton));
    head.appendChild(copyButton);
    card.appendChild(head);

    const metricGrid = el('div', 'details-metric-grid');
    for (const metric of metrics) {
      const cell = el('div', 'details-metric');
      cell.append(
        el('span', 'details-metric-label', metric.label),
        el('span', `details-metric-value${metric.tone ? ` ${metric.tone}` : ''}`, metric.value),
      );
      metricGrid.appendChild(cell);
    }
    card.appendChild(metricGrid);
    return card;
  }

  private cpuMetrics(cpu: CpuDetails): Metric[] {
    const metrics: Metric[] = [
      { label: 'Physical cores', value: String(cpu.physical_cores) },
      { label: 'Threads', value: String(cpu.logical_processors) },
      { label: 'CPU load', value: cpu.load_percent !== null ? `${cpu.load_percent}%` : 'Not reported', tone: cpu.load_percent !== null ? undefined : 'muted' },
      clockMetric(cpu.current_clock_mhz, cpu.max_clock_mhz),
    ];
    if (cpu.temperature_c !== null) metrics.push(temperatureMetric(cpu.temperature_c));
    return metrics;
  }

  private gpuMetrics(gpu: GpuDetails): Metric[] {
    const vram = gpu.vram_total_mb
      ? gpu.vram_used_mb !== null
        ? `${gb(gpu.vram_used_mb)} / ${gb(gpu.vram_total_mb)} GB`
        : `${gb(gpu.vram_total_mb)} GB`
      : 'Not reported';
    const metrics: Metric[] = [
      { label: 'VRAM', value: vram, tone: gpu.vram_total_mb ? undefined : 'muted' },
      { label: 'GPU load', value: gpu.utilization_percent !== null ? `${gpu.utilization_percent}%` : 'Not reported', tone: gpu.utilization_percent !== null ? undefined : 'muted' },
      { label: 'Driver', value: gpu.driver_version || 'Not reported', tone: gpu.driver_version ? undefined : 'muted' },
    ];
    if (gpu.temperature_c !== null) metrics.splice(2, 0, temperatureMetric(gpu.temperature_c));
    return metrics;
  }

  private unavailableGpuCard(): HTMLElement {
    const card = el('article', 'details-hardware-card details-empty-card');
    card.append(
      el('span', 'details-kind details-kind-gpu', 'GPU'),
      el('span', 'details-empty-title', 'No graphics adapter reported'),
      el('span', 'details-empty-copy', 'Windows did not return a display adapter for this session.'),
    );
    return card;
  }

  private memoryCard(details: SystemDetails): HTMLElement {
    const {
      total_mb: total,
      available_mb: available,
      module_count: modules,
      memory_type: memoryType,
      configured_speed_mt_s: speed,
    } = details.memory;
    const used = Math.max(0, total - available);
    const percent = total > 0 ? Math.round((used / total) * 100) : 0;
    const card = el('article', 'details-summary-card');
    card.append(
      el('span', 'details-summary-label', 'MEMORY'),
      el('strong', 'details-summary-value details-memory-title', `${memoryType} · ${gb(total)} GB installed`),
    );
    const gauge = el('div', 'details-memory-gauge');
    const fill = el('span', `details-memory-fill${percent >= 85 ? ' hot' : ''}`);
    fill.style.width = `${percent}%`;
    gauge.appendChild(fill);
    card.appendChild(gauge);
    const meta = el('div', 'details-summary-meta details-memory-meta');
    meta.append(
      summaryItem('In use', `${gb(used)} GB`),
      summaryItem('Available', `${gb(available)} GB`),
      summaryItem('Modules', String(modules)),
      summaryItem('Configured speed', speed ? `${speed} MT/s` : 'Not reported'),
    );
    card.appendChild(meta);
    return card;
  }

  private environmentCard(details: SystemDetails): HTMLElement {
    const card = el('article', 'details-summary-card');
    card.append(
      el('span', 'details-summary-label', 'WINDOWS & POWER'),
      el('strong', 'details-power-plan', details.active_power_plan),
    );
    const rows = el('div', 'details-environment-rows');
    rows.append(
      environmentRow('Operating system', details.os_name),
      environmentRow('Build', details.os_build),
    );
    card.appendChild(rows);
    return card;
  }

  private async copyName(name: string, kind: string, button: HTMLButtonElement): Promise<void> {
    try {
      await navigator.clipboard.writeText(name);
    } catch {
      const textarea = document.createElement('textarea');
      textarea.value = name;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      textarea.remove();
    }
    button.classList.add('copied');
    this.notify(`${kind} name copied`);
    setTimeout(() => button.classList.remove('copied'), 900);
  }
}

function temperatureMetric(value: number): Metric {
  return {
    label: 'Temperature',
    value: `${value.toFixed(1)} °C`,
    tone: value >= 90 ? 'hot' : value >= 75 ? 'warm' : 'good',
  };
}

function clockMetric(currentMhz: number | null, maximumMhz: number | null): Metric {
  if (currentMhz !== null && maximumMhz !== null) {
    return {
      label: 'Clock / maximum',
      value: `${ghz(currentMhz)} / ${ghz(maximumMhz)} GHz`,
    };
  }
  if (currentMhz !== null) return { label: 'Current clock', value: `${ghz(currentMhz)} GHz` };
  if (maximumMhz !== null) return { label: 'Maximum clock', value: `${ghz(maximumMhz)} GHz` };
  return { label: 'Clock', value: 'Not reported', tone: 'muted' };
}

function ghz(megahertz: number): string {
  return (megahertz / 1000).toFixed(2);
}

function gb(megabytes: number): string {
  return (megabytes / 1024).toFixed(1);
}

function summaryItem(label: string, value: string): HTMLElement {
  const item = el('span', 'details-summary-item');
  item.append(el('small', '', label), el('b', '', value));
  return item;
}

function environmentRow(label: string, value: string): HTMLElement {
  const row = el('div', 'details-environment-row');
  row.append(el('span', '', label), el('strong', '', value));
  return row;
}

function copyIcon(): SVGElement {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'details-copy-icon');
  svg.setAttribute('width', '13');
  svg.setAttribute('height', '13');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '2');
  const rect = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
  rect.setAttribute('x', '9');
  rect.setAttribute('y', '9');
  rect.setAttribute('width', '11');
  rect.setAttribute('height', '11');
  rect.setAttribute('rx', '2');
  const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  path.setAttribute('d', 'M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1');
  svg.append(rect, path);
  return svg;
}

function el(tag: string, className = '', text = ''): HTMLElement {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}
