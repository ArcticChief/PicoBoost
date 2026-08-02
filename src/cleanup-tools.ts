import { api } from './api';
import { confirmDialog } from './dialogs';
import { CleanupCategory, CleanupGroup } from './types';

type Notify = (message: string) => void;

const GROUPS: CleanupGroup[] = ['Everyday', 'Developer', 'Advanced'];
const GROUP_HELP: Record<CleanupGroup, string> = {
  Everyday: 'Routine, low-impact maintenance',
  Developer: 'Re-downloadable package and tooling data',
  Advanced: 'May affect the next game launch or project build',
};

/** Scan-first cleanup UI. It can submit only category IDs returned by Rust. */
export class CleanupToolsModal {
  private readonly overlay = document.getElementById('cleanup-overlay') as HTMLElement;
  private readonly content = document.getElementById('cleanup-content') as HTMLElement;
  private readonly closeButton = document.getElementById('cleanup-close') as HTMLButtonElement;
  private readonly scanButton = document.getElementById('cleanup-rescan') as HTMLButtonElement;
  private readonly cleanButton = document.getElementById('cleanup-run') as HTMLButtonElement;
  private readonly selectionText = document.getElementById('cleanup-selection') as HTMLElement;
  private readonly foundSize = document.getElementById('cleanup-found-size') as HTMLElement;
  private readonly foundMeta = document.getElementById('cleanup-found-meta') as HTMLElement;
  private readonly riskNotice = document.getElementById('cleanup-selection-risk') as HTMLElement;
  private readonly recommendedButton = document.getElementById('cleanup-recommended') as HTMLButtonElement;
  private readonly checkAllButton = document.getElementById('cleanup-check-all') as HTMLButtonElement;
  private readonly clearAllButton = document.getElementById('cleanup-clear-all') as HTMLButtonElement;
  private categories: CleanupCategory[] = [];
  private selected = new Set<string>();
  private scanning = false;
  private cleaning = false;
  private hasScanned = false;
  private requestId = 0;
  private previousFocus: HTMLElement | null = null;

  constructor(private readonly notify: Notify) {
    document.getElementById('cleanup-tools-btn')?.addEventListener('click', () => void this.open());
    this.closeButton.addEventListener('click', () => this.close());
    this.scanButton.addEventListener('click', () => void this.scan(true));
    this.cleanButton.addEventListener('click', () => void this.clean());
    this.recommendedButton.addEventListener('click', () => this.selectRecommended());
    this.checkAllButton.addEventListener('click', () => this.selectAll());
    this.clearAllButton.addEventListener('click', () => this.clearSelection());
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
    await this.scan(false);
  }

  private close(): void {
    if (this.cleaning) return;
    this.requestId += 1;
    this.scanning = false;
    this.setBusy(false);
    this.overlay.classList.add('hidden');
    this.overlay.setAttribute('aria-hidden', 'true');
    this.previousFocus?.focus();
  }

  private async scan(preserveSelection: boolean): Promise<void> {
    if (this.cleaning) return;
    this.scanning = true;
    this.setBusy(true);
    this.renderLoading();
    this.foundSize.textContent = 'Scanning…';
    this.foundMeta.textContent = 'Checking approved locations';
    const requestId = ++this.requestId;
    try {
      const categories = await api.scanCleanup();
      if (requestId !== this.requestId) return;
      const oldSelection = new Set(this.selected);
      this.categories = categories;
      this.selected.clear();
      for (const category of categories) {
        const shouldSelect = preserveSelection || this.hasScanned
          ? oldSelection.has(category.id)
          : category.default_selected;
        if (shouldSelect && category.available && category.files > 0) this.selected.add(category.id);
      }
      this.hasScanned = true;
      this.renderCategories();
    } catch (error) {
      if (requestId === this.requestId) {
        this.categories = [];
        this.selected.clear();
        this.hasScanned = false;
        this.renderError(String(error));
      }
    } finally {
      if (requestId === this.requestId) {
        this.scanning = false;
        this.setBusy(false);
        this.updateSelection();
      }
    }
  }

  private async clean(): Promise<void> {
    if (this.cleaning || this.scanning || this.selected.size === 0) return;
    const chosen = this.categories.filter((category) => this.selected.has(category.id));
    const bytes = chosen.reduce((total, category) => total + category.bytes, 0);
    const permanent = chosen.some((category) => category.id === 'recycle_bin');
    const advanced = chosen.some((category) => category.group === 'Advanced');
    const qualifiers = [
      permanent ? 'Recycle Bin items will be permanently removed.' : '',
      advanced ? 'Advanced caches may need to be downloaded or rebuilt.' : '',
    ].filter(Boolean).join(' ');
    const confirmed = await confirmDialog(
      `Remove ${formatBytes(bytes)} across ${chosen.length} selected ${chosen.length === 1 ? 'category' : 'categories'}? ${qualifiers}`.trim(),
      { title: 'Clean selected files?', confirmText: 'Clean now', danger: permanent || advanced },
    );
    if (!confirmed) return;

    this.cleaning = true;
    this.setBusy(true);
    this.content.classList.add('cleanup-is-running');
    try {
      const result = await api.runCleanup(chosen.map((category) => category.id));
      const skipped = result.failed_items > 0 ? ` · ${result.failed_items} locked or unavailable skipped` : '';
      this.notify(`Freed ${formatBytes(result.bytes_freed)} · ${result.files_removed.toLocaleString()} files${skipped}`);
      this.selected.clear();
      this.hasScanned = false;
      this.cleaning = false;
      this.content.classList.remove('cleanup-is-running');
      await this.scan(false);
    } catch (error) {
      this.notify(`Cleanup stopped: ${String(error)}`);
    } finally {
      this.cleaning = false;
      this.content.classList.remove('cleanup-is-running');
      this.setBusy(false);
      this.updateSelection();
    }
  }

  private renderLoading(): void {
    this.content.replaceChildren();
    const loading = make('div', 'cleanup-loading');
    loading.append(make('span', 'details-spinner'), make('span', '', 'Scanning approved cache locations…'));
    this.content.appendChild(loading);
  }

  private renderError(message: string): void {
    this.content.replaceChildren();
    const error = make('div', 'cleanup-error');
    error.append(
      make('strong', '', 'Could not scan cleanup locations'),
      make('span', '', message),
    );
    this.content.appendChild(error);
  }

  private renderCategories(): void {
    this.content.replaceChildren();
    for (const group of GROUPS) {
      const categories = this.categories.filter((category) => category.group === group);
      if (!categories.length) continue;
      const section = make('section', 'cleanup-group');
      const heading = make('div', 'cleanup-group-heading');
      const headingCopy = make('div', 'cleanup-group-copy');
      headingCopy.append(make('h3', '', group), make('span', '', GROUP_HELP[group]));
      const available = categories.filter(isSelectable);
      const groupBytes = available.reduce((total, category) => total + category.bytes, 0);
      const groupActions = make('div', 'cleanup-group-actions');
      groupActions.append(make('span', 'cleanup-group-total', `${formatBytes(groupBytes)} · ${available.length} found`));
      const groupButton = make('button', 'cleanup-group-select', 'Check group') as HTMLButtonElement;
      groupButton.type = 'button';
      groupButton.dataset.group = group;
      groupButton.disabled = available.length === 0;
      groupButton.addEventListener('click', () => this.toggleGroup(group));
      groupActions.appendChild(groupButton);
      heading.append(headingCopy, groupActions);
      section.appendChild(heading);

      const list = make('div', 'cleanup-list');
      categories.forEach((category) => list.appendChild(this.categoryCard(category)));
      section.appendChild(list);
      this.content.appendChild(section);
    }
  }

  private categoryCard(category: CleanupCategory): HTMLElement {
    const label = make('label', `cleanup-item${category.available && category.files > 0 ? '' : ' unavailable'}`);
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.dataset.categoryId = category.id;
    checkbox.checked = this.selected.has(category.id);
    checkbox.disabled = !category.available || category.files === 0;
    checkbox.addEventListener('change', () => {
      if (checkbox.checked) this.selected.add(category.id);
      else this.selected.delete(category.id);
      this.updateSelection();
    });
    const control = make('span', 'cleanup-check');
    const copy = make('span', 'cleanup-item-copy');
    const nameRow = make('span', 'cleanup-name-row');
    nameRow.append(make('strong', '', category.name));
    if (category.caution) nameRow.append(make('span', 'cleanup-caution-badge', category.group === 'Advanced' ? 'Advanced' : 'Note'));
    copy.append(nameRow, make('span', 'cleanup-description', category.description));
    if (category.caution) copy.appendChild(make('span', 'cleanup-caution', category.caution));
    const amount = make('span', 'cleanup-amount');
    amount.append(
      make('strong', '', formatBytes(category.bytes)),
      make('span', '', `${category.files.toLocaleString()} ${category.files === 1 ? 'file' : 'files'}`),
    );
    label.append(checkbox, control, copy, amount);
    return label;
  }

  private updateSelection(): void {
    const selectable = this.categories.filter(isSelectable);
    const chosen = this.categories.filter((category) => this.selected.has(category.id));
    const bytes = chosen.reduce((total, category) => total + category.bytes, 0);
    const totalBytes = selectable.reduce((total, category) => total + category.bytes, 0);
    const totalFiles = selectable.reduce((total, category) => total + category.files, 0);
    this.selectionText.textContent = chosen.length
      ? `${chosen.length} selected · ${formatBytes(bytes)}`
      : 'Nothing selected';
    this.foundSize.textContent = formatBytes(totalBytes);
    this.foundMeta.textContent = totalFiles
      ? `${totalFiles.toLocaleString()} ${totalFiles === 1 ? 'file' : 'files'} in ${selectable.length} ${selectable.length === 1 ? 'category' : 'categories'}`
      : this.hasScanned ? 'System is already tidy in these locations' : 'Scan unavailable';

    const busy = this.scanning || this.cleaning;
    const recommended = selectable.filter((category) => category.default_selected);
    const recommendedSelected = recommended.length > 0
      && chosen.length === recommended.length
      && recommended.every((category) => this.selected.has(category.id));
    const allSelected = selectable.length > 0 && chosen.length === selectable.length;
    this.recommendedButton.disabled = busy || recommended.length === 0;
    this.recommendedButton.setAttribute('aria-pressed', String(recommendedSelected));
    this.checkAllButton.disabled = busy || selectable.length === 0 || allSelected;
    this.checkAllButton.setAttribute('aria-pressed', String(allSelected));
    this.clearAllButton.disabled = busy || chosen.length === 0;
    this.cleanButton.disabled = this.scanning || this.cleaning || chosen.length === 0;
    this.updateGroupButtons();
    this.updateRiskNotice(chosen);
  }

  private selectRecommended(): void {
    this.selected = new Set(
      this.categories
        .filter((category) => isSelectable(category) && category.default_selected)
        .map((category) => category.id),
    );
    this.syncCheckboxes();
    this.updateSelection();
  }

  private selectAll(): void {
    this.selected = new Set(this.categories.filter(isSelectable).map((category) => category.id));
    this.syncCheckboxes();
    this.updateSelection();
  }

  private clearSelection(): void {
    this.selected.clear();
    this.syncCheckboxes();
    this.updateSelection();
  }

  private toggleGroup(group: CleanupGroup): void {
    const groupCategories = this.categories.filter((category) => category.group === group && isSelectable(category));
    const allSelected = groupCategories.length > 0 && groupCategories.every((category) => this.selected.has(category.id));
    for (const category of groupCategories) {
      if (allSelected) this.selected.delete(category.id);
      else this.selected.add(category.id);
    }
    this.syncCheckboxes();
    this.updateSelection();
  }

  private syncCheckboxes(): void {
    this.content.querySelectorAll<HTMLInputElement>('input[data-category-id]').forEach((checkbox) => {
      checkbox.checked = this.selected.has(checkbox.dataset.categoryId ?? '');
    });
  }

  private updateGroupButtons(): void {
    this.content.querySelectorAll<HTMLButtonElement>('.cleanup-group-select').forEach((button) => {
      const group = button.dataset.group as CleanupGroup;
      const categories = this.categories.filter((category) => category.group === group && isSelectable(category));
      const allSelected = categories.length > 0 && categories.every((category) => this.selected.has(category.id));
      button.textContent = allSelected ? 'Clear group' : 'Check group';
      button.setAttribute('aria-pressed', String(allSelected));
      button.disabled = this.scanning || this.cleaning || categories.length === 0;
    });
  }

  private updateRiskNotice(chosen: CleanupCategory[]): void {
    const includesRecycleBin = chosen.some((category) => category.id === 'recycle_bin');
    const includesAdvanced = chosen.some((category) => category.group === 'Advanced');
    const includesDeveloper = chosen.some((category) => category.group === 'Developer');
    this.riskNotice.className = 'cleanup-selection-risk';
    if (includesRecycleBin) {
      this.riskNotice.classList.add('danger');
      this.riskNotice.textContent = 'Permanent selection: Recycle Bin items cannot be restored after cleaning.';
    } else if (includesAdvanced) {
      this.riskNotice.classList.add('warning');
      this.riskNotice.textContent = 'Advanced selection: games may rebuild shaders and projects may restore packages again.';
    } else if (includesDeveloper) {
      this.riskNotice.classList.add('info');
      this.riskNotice.textContent = 'Developer caches are safe to recreate but package tools will download them again.';
    } else {
      this.riskNotice.classList.add('hidden');
      this.riskNotice.textContent = '';
    }
  }

  private setBusy(busy: boolean): void {
    this.scanButton.disabled = busy;
    this.recommendedButton.disabled = busy;
    this.checkAllButton.disabled = busy;
    this.clearAllButton.disabled = busy;
    this.closeButton.disabled = this.cleaning;
    if (this.cleaning) this.cleanButton.textContent = 'Cleaning…';
    else this.cleanButton.textContent = 'Clean selected';
  }
}

function isSelectable(category: CleanupCategory): boolean {
  return category.available && category.files > 0;
}

function make(tag: string, className = '', text = ''): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`;
}
