import { api } from './api';
import { confirmDialog } from './dialogs';
import { SoftwareEntry } from './types';

type Notify = (message: string) => void;

function make(tag: string, className = '', text = ''): HTMLElement {
  const el = document.createElement(tag);
  if (className) el.className = className;
  if (text) el.textContent = text;
  return el;
}

function formatSize(mb: number): string {
  if (!mb || mb <= 0) return '—';
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${mb} MB`;
}

/**
 * Installed-software list. Uninstalling launches the program's own uninstaller
 * (via the backend, which re-reads the command from the registry); leftover
 * removal deletes only a dead Add/Remove Programs entry after the backend
 * re-confirms the program's files are gone.
 */
export class SoftwareManagerModal {
  private readonly overlay = document.getElementById('software-overlay') as HTMLElement;
  private readonly content = document.getElementById('software-content') as HTMLElement;
  private readonly closeButton = document.getElementById('software-close') as HTMLButtonElement;
  private readonly refreshButton = document.getElementById('software-refresh') as HTMLButtonElement;
  private readonly searchInput = document.getElementById('software-search') as HTMLInputElement;
  private readonly orphansOnly = document.getElementById('software-orphans-only') as HTMLInputElement;
  private readonly countText = document.getElementById('software-count') as HTMLElement;
  private readonly summaryText = document.getElementById('software-summary') as HTMLElement;
  private readonly sortSelect = document.getElementById('software-sort') as HTMLSelectElement;
  private entries: SoftwareEntry[] = [];
  private sortKey: 'name' | 'size' | 'date' = 'name';
  private loading = false;
  private busy = false;
  private requestId = 0;
  private previousFocus: HTMLElement | null = null;

  constructor(private readonly notify: Notify) {
    document.getElementById('software-tools-btn')?.addEventListener('click', () => void this.open());
    this.closeButton.addEventListener('click', () => this.close());
    this.refreshButton.addEventListener('click', () => void this.load());
    this.searchInput.addEventListener('input', () => this.render());
    this.orphansOnly.addEventListener('change', () => this.render());
    this.sortSelect.addEventListener('change', () => {
      this.sortKey = this.sortSelect.value as 'name' | 'size' | 'date';
      this.render();
    });
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

  private async open(): Promise<void> {
    if (!this.overlay.classList.contains('hidden')) return;
    this.previousFocus = document.activeElement as HTMLElement | null;
    this.overlay.classList.remove('hidden');
    this.overlay.setAttribute('aria-hidden', 'false');
    this.closeButton.focus();
    await this.load();
  }

  private close(): void {
    if (this.busy) return;
    this.requestId += 1;
    this.overlay.classList.add('hidden');
    this.overlay.setAttribute('aria-hidden', 'true');
    this.previousFocus?.focus();
  }

  private async load(): Promise<void> {
    if (this.busy) return;
    this.loading = true;
    this.renderLoading();
    this.countText.textContent = 'Loading…';
    const requestId = ++this.requestId;
    try {
      const entries = await api.listInstalledSoftware();
      if (requestId !== this.requestId) return;
      this.entries = entries;
      this.loading = false;
      this.render();
    } catch (error) {
      if (requestId !== this.requestId) return;
      this.loading = false;
      this.content.innerHTML = '';
      this.content.appendChild(make('div', 'software-empty', `Could not read installed software: ${String(error)}`));
      this.countText.textContent = '—';
      this.summaryText.textContent = 'Error';
    }
  }

  private filtered(): SoftwareEntry[] {
    const query = this.searchInput.value.trim().toLowerCase();
    const orphansOnly = this.orphansOnly.checked;
    return this.entries.filter((entry) => {
      if (orphansOnly && !entry.orphaned) return false;
      if (!query) return true;
      return entry.name.toLowerCase().includes(query) || entry.publisher.toLowerCase().includes(query);
    });
  }

  private sortRows(rows: SoftwareEntry[]): SoftwareEntry[] {
    const sorted = [...rows];
    if (this.sortKey === 'size') {
      sorted.sort((a, b) => (b.size_mb || 0) - (a.size_mb || 0));
    } else if (this.sortKey === 'date') {
      sorted.sort((a, b) => (b.install_date || '').localeCompare(a.install_date || ''));
    } else {
      sorted.sort((a, b) => a.name.localeCompare(b.name));
    }
    return sorted;
  }

  private render(): void {
    if (this.loading) return;
    const rows = this.sortRows(this.filtered());
    const orphanCount = this.entries.filter((entry) => entry.orphaned).length;
    const shownMb = rows.reduce((total, entry) => total + (entry.size_mb || 0), 0);
    this.countText.textContent = `${rows.length} shown · ${formatSize(shownMb)}`;
    this.summaryText.textContent = `${this.entries.length} installed${
      orphanCount ? ` · ${orphanCount} leftover${orphanCount === 1 ? '' : 's'}` : ''
    }`;

    this.content.innerHTML = '';
    if (rows.length === 0) {
      const message = this.entries.length === 0 ? 'No installed software found.' : 'Nothing matches your filter.';
      this.content.appendChild(make('div', 'software-empty', message));
      return;
    }

    const list = make('div', 'software-list');
    for (const entry of rows) {
      list.appendChild(this.rowFor(entry));
    }
    this.content.appendChild(list);
  }

  private rowFor(entry: SoftwareEntry): HTMLElement {
    const row = make('div', `software-row${entry.orphaned ? ' is-orphan' : ''}`);
    if (entry.location) row.title = entry.location;

    const info = make('div', 'software-info');
    const nameRow = make('div', 'software-name-row');
    nameRow.appendChild(make('span', 'software-name', entry.name));
    if (entry.orphaned) nameRow.appendChild(make('span', 'software-badge', 'Leftover'));
    if (entry.scope === 'user') nameRow.appendChild(make('span', 'software-badge subtle', 'User'));
    info.appendChild(nameRow);

    const metaParts: string[] = [];
    if (entry.publisher) metaParts.push(entry.publisher);
    if (entry.version) metaParts.push(`v${entry.version}`);
    if (entry.install_date) metaParts.push(entry.install_date);
    info.appendChild(make('div', 'software-meta', metaParts.join('  ·  ')));
    row.appendChild(info);

    row.appendChild(make('span', 'software-size', formatSize(entry.size_mb)));

    const actions = make('div', 'software-actions');
    if (entry.orphaned) {
      const remove = make('button', 'software-btn danger', 'Remove leftover') as HTMLButtonElement;
      remove.type = 'button';
      remove.addEventListener('click', () => void this.removeLeftover(entry, remove));
      actions.appendChild(remove);
    } else {
      const uninstall = make('button', 'software-btn', 'Uninstall') as HTMLButtonElement;
      uninstall.type = 'button';
      uninstall.disabled = !entry.has_uninstall;
      if (!entry.has_uninstall) uninstall.title = 'This program does not provide an uninstaller';
      uninstall.addEventListener('click', () => void this.uninstall(entry, uninstall));
      actions.appendChild(uninstall);
    }
    row.appendChild(actions);
    return row;
  }

  private async uninstall(entry: SoftwareEntry, button: HTMLButtonElement): Promise<void> {
    const confirmed = await confirmDialog(
      `Run the uninstaller for “${entry.name}”? Its own uninstall program will open so you can finish there.`,
      { title: 'Uninstall software', confirmText: 'Uninstall', danger: true },
    );
    if (!confirmed) return;
    this.busy = true;
    button.disabled = true;
    try {
      this.notify(await api.uninstallSoftware(entry.id));
    } catch (error) {
      this.notify(String(error));
      button.disabled = false;
    } finally {
      this.busy = false;
    }
  }

  private async removeLeftover(entry: SoftwareEntry, button: HTMLButtonElement): Promise<void> {
    const confirmed = await confirmDialog(
      `Remove the leftover entry for “${entry.name}”? Its files are already gone; this only clears the dead Add/Remove Programs registry entry.`,
      { title: 'Remove leftover entry', confirmText: 'Remove entry', danger: true },
    );
    if (!confirmed) return;
    this.busy = true;
    button.disabled = true;
    try {
      this.notify(await api.removeSoftwareLeftover(entry.id));
      this.entries = this.entries.filter((item) => item.id !== entry.id);
      this.render();
    } catch (error) {
      this.notify(String(error));
      button.disabled = false;
    } finally {
      this.busy = false;
    }
  }

  private renderLoading(): void {
    this.content.innerHTML = '';
    const loading = make('div', 'cleanup-loading');
    loading.appendChild(make('span', 'details-spinner'));
    loading.appendChild(make('span', '', 'Reading installed software…'));
    this.content.appendChild(loading);
  }
}
