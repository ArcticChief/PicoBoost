import { api } from './api';
import { confirmDialog } from './dialogs';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { StorageItem, StorageScanResult, StorageSearchResult } from './types';

type Notify = (message: string) => void;
type StorageTab = 'children' | 'largest';
type StorageMapView = 'folders' | 'files';
type StorageBusyPhase = 'picker' | 'scan' | 'recycle';

interface TreemapRect {
  item: StorageItem | null;
  label: string;
  bytes: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

interface FileTone {
  key: string;
  label: string;
  color: string;
}

/** Visual, session-bound folder analyzer with explicit Recycle Bin actions. */
export class StorageMapModal {
  private readonly overlay = document.getElementById('storage-map-overlay') as HTMLElement;
  private readonly dialog = this.overlay.querySelector('.storage-map-dialog') as HTMLElement;
  private readonly emptyState = document.getElementById('storage-map-empty') as HTMLElement;
  private readonly workspace = document.getElementById('storage-map-workspace') as HTMLElement;
  private readonly closeButton = document.getElementById('storage-map-close') as HTMLButtonElement;
  private readonly minimizeButton = document.getElementById('storage-map-minimize') as HTMLButtonElement;
  private readonly cancelButton = document.getElementById('storage-map-cancel') as HTMLButtonElement;
  private readonly progress = document.getElementById('storage-map-progress') as HTMLElement;
  private readonly progressTitle = document.getElementById('storage-map-progress-title') as HTMLElement;
  private readonly progressPath = document.getElementById('storage-map-progress-path') as HTMLElement;
  private readonly progressCount = document.getElementById('storage-map-progress-count') as HTMLElement;
  private readonly progressElapsed = document.getElementById('storage-map-progress-elapsed') as HTMLElement;
  private readonly progressHelp = document.getElementById('storage-map-progress-help') as HTMLElement;
  private readonly subtitle = document.getElementById('storage-map-subtitle') as HTMLElement;
  private readonly chooseButtons = [
    document.getElementById('storage-map-choose-empty') as HTMLButtonElement,
    document.getElementById('storage-map-choose') as HTMLButtonElement,
  ];
  private readonly upButton = document.getElementById('storage-map-up') as HTMLButtonElement;
  private readonly rescanButton = document.getElementById('storage-map-rescan') as HTMLButtonElement;
  private readonly recycleButton = document.getElementById('storage-map-recycle') as HTMLButtonElement;
  private readonly recycleLabel = document.getElementById('storage-map-recycle-label') as HTMLElement;
  private readonly searchInput = document.getElementById('storage-map-search') as HTMLInputElement;
  private readonly treemap = document.getElementById('storage-treemap') as HTMLCanvasElement;
  private readonly visualMeta = document.getElementById('storage-map-visual-meta') as HTMLElement;
  private readonly legend = document.getElementById('storage-map-legend') as HTMLElement;
  private readonly list = document.getElementById('storage-map-list') as HTMLElement;
  private readonly selectionText = document.getElementById('storage-map-selection') as HTMLElement;
  private scan: StorageScanResult | null = null;
  private selected = new Set<string>();
  private activeTab: StorageTab = 'children';
  private activeMapView: StorageMapView = 'folders';
  private busy = false;
  private recycling = false;
  private busyPhase: StorageBusyPhase = 'scan';
  private progressTimer: ReturnType<typeof setInterval> | null = null;
  private progressStartedAt = 0;
  private progressPollActive = false;
  private progressCycle = 0;
  private requestId = 0;
  private searchResult: StorageSearchResult | null = null;
  private searchTimer: ReturnType<typeof setTimeout> | null = null;
  private searchRequestId = 0;
  private treemapRects: TreemapRect[] = [];
  private collapsed = false;
  private previousFocus: HTMLElement | null = null;

  constructor(private readonly notify: Notify) {
    document.getElementById('storage-map-btn')?.addEventListener('click', () => this.open());
    this.closeButton.addEventListener('click', () => this.close());
    this.minimizeButton.addEventListener('click', () => this.setCollapsed(!this.collapsed));
    this.cancelButton.addEventListener('click', () => void this.cancelScan(true));
    this.chooseButtons.forEach((button) => button.addEventListener('click', () => void this.chooseFolder()));
    this.rescanButton.addEventListener('click', () => void this.runScan(() => api.rescanStorage()));
    this.upButton.addEventListener('click', () => void this.navigateIndex(() => api.storageGoUp()));
    this.recycleButton.addEventListener('click', () => void this.recycleSelected());
    this.searchInput.addEventListener('input', () => this.scheduleSearch());
    document.querySelectorAll<HTMLButtonElement>('[data-storage-tab]').forEach((button) => {
      button.addEventListener('click', () => this.setTab(button.dataset.storageTab as StorageTab));
    });
    document.querySelectorAll<HTMLButtonElement>('[data-storage-map-view]').forEach((button) => {
      button.addEventListener('click', () => this.setMapView(button.dataset.storageMapView as StorageMapView));
    });
    this.treemap.addEventListener('pointermove', (event) => this.describeTreemapPoint(event));
    this.treemap.addEventListener('pointerleave', () => {
      this.treemap.title = '';
      this.treemap.style.cursor = 'default';
    });
    this.treemap.addEventListener('click', (event) => {
      const rect = this.treemapRectAt(event);
      if (!rect?.item) return;
      if (rect.item.is_directory) void this.openFolder(rect.item);
      else this.revealItem(rect.item, this.activeMapView === 'files' ? 'largest' : 'children');
    });
    window.addEventListener('resize', () => {
      if (this.scan && !this.overlay.classList.contains('hidden') && !this.collapsed) this.renderTreemap();
    });
    document.addEventListener('pointerdown', (event) => {
      if (
        !this.overlay.classList.contains('hidden') &&
        !this.collapsed &&
        event.target instanceof Node &&
        !this.dialog.contains(event.target)
      ) {
        // The event continues to its original target; Storage Map simply gets
        // out of the way before the user interacts with the main application.
        this.setCollapsed(true);
      }
    }, true);
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

  private open(): void {
    if (!this.overlay.classList.contains('hidden')) {
      if (this.collapsed) {
        this.setCollapsed(false);
        this.closeButton.focus();
      }
      return;
    }
    this.previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.overlay.classList.remove('hidden');
    this.overlay.setAttribute('aria-hidden', 'false');
    this.setCollapsed(false);
    this.renderSurface();
    this.closeButton.focus();
  }

  private close(): void {
    if (this.recycling) return;
    if (this.busy) void api.cancelStorageScan();
    this.requestId += 1;
    this.setBusy(false);
    this.overlay.classList.add('hidden');
    this.overlay.setAttribute('aria-hidden', 'true');
    this.previousFocus?.focus();
  }

  private setCollapsed(collapsed: boolean): void {
    this.collapsed = collapsed;
    this.overlay.classList.toggle('collapsed', collapsed);
    this.minimizeButton.setAttribute('aria-pressed', String(collapsed));
    this.minimizeButton.title = collapsed ? 'Expand storage map' : 'Collapse storage map';
    this.minimizeButton.setAttribute('aria-label', this.minimizeButton.title);
    if (!collapsed && this.scan) requestAnimationFrame(() => this.renderTreemap());
  }

  private async cancelScan(showNotice: boolean): Promise<void> {
    if (!this.busy || this.recycling) return;
    this.requestId += 1;
    this.setBusy(false);
    try {
      await api.cancelStorageScan();
      if (showNotice) this.notify('Storage scan cancelled');
    } catch (error) {
      if (showNotice) this.notify(`Could not cancel storage scan: ${String(error)}`);
    }
  }

  private async chooseFolder(): Promise<void> {
    if (this.busy) return;
    const requestId = ++this.requestId;
    this.setBusy(true, 'Choose a folder in the Windows dialog', 'picker');
    await nextPaint();
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: 'Choose a folder to understand',
      });
      if (requestId !== this.requestId || typeof selected !== 'string') return;
      const fastMode = await api.storageFastModeSupport(selected);
      if (requestId !== this.requestId) return;
      let elevateFastScan = false;
      if (fastMode.requires_elevation) {
        this.setBusy(false);
        elevateFastScan = await confirmDialog(
          'PicoBoost can read the NTFS file index once instead of opening every folder. Windows will ask for administrator approval for a short-lived, read-only scanner; the main app stays non-admin. Cancel uses the slower folder scan.',
          { title: 'Use the fast NTFS scanner?', confirmText: 'Use fast scan' },
        );
        if (requestId !== this.requestId) return;
      }
      const scansWholeDrive = /^[a-z]:[\\/]?$/i.test(selected.trim());
      this.setBusy(
        true,
        fastMode.available || elevateFastScan
          ? scansWholeDrive ? 'Streaming the NTFS Master File Table…' : 'Reading folder records directly from NTFS…'
          : 'Building the folder index…',
        'scan',
        selected,
      );
      this.progressHelp.textContent = fastMode.available || elevateFastScan
        ? 'The fast scanner reads file records sequentially and never opens file contents.'
        : fastMode.reason;
      await nextPaint();
      const result = elevateFastScan
        ? await api.scanStorageFolderFast(selected)
        : await api.scanStorageFolder(selected);
      if (requestId !== this.requestId) return;
      this.applyScan(result);
    } catch (error) {
      if (requestId === this.requestId) this.notify(`Storage scan stopped: ${String(error)}`);
    } finally {
      if (requestId === this.requestId) this.setBusy(false);
    }
  }

  private async runScan(action: () => Promise<StorageScanResult>): Promise<void> {
    if (this.busy) return;
    const requestId = ++this.requestId;
    this.setBusy(true, 'Measuring files and folders…', 'scan', this.scan?.current ?? 'Selected folder');
    await nextPaint();
    try {
      const result = await action();
      if (requestId === this.requestId) this.applyScan(result);
    } catch (error) {
      if (requestId === this.requestId) this.notify(`Storage scan stopped: ${String(error)}`);
    } finally {
      if (requestId === this.requestId) this.setBusy(false);
    }
  }

  private async navigateIndex(action: () => Promise<StorageScanResult>): Promise<void> {
    if (this.busy) return;
    const requestId = ++this.requestId;
    this.list.replaceChildren(element('div', 'storage-loading', 'Opening from the in-memory index…'));
    try {
      const result = await action();
      if (requestId === this.requestId) this.applyScan(result);
    } catch (error) {
      if (requestId === this.requestId) {
        this.renderList();
        this.notify(`Could not open indexed folder: ${String(error)}`);
      }
    }
  }

  private applyScan(scan: StorageScanResult): void {
    this.scan = scan;
    this.selected.clear();
    this.searchInput.value = '';
    this.searchResult = null;
    this.searchRequestId += 1;
    if (this.searchTimer) clearTimeout(this.searchTimer);
    this.searchTimer = null;
    this.renderSurface();
  }

  private renderSurface(): void {
    this.emptyState.classList.toggle('hidden', this.scan !== null);
    this.workspace.classList.toggle('hidden', this.scan === null);
    if (!this.scan) {
      this.updateSelection();
      return;
    }

    const path = document.getElementById('storage-map-path') as HTMLElement;
    path.textContent = this.scan.current;
    path.title = this.scan.current;
    setText('storage-map-total', formatBytes(this.scan.total_bytes));
    setText('storage-map-files', this.scan.files.toLocaleString());
    setText('storage-map-folders', this.scan.folders.toLocaleString());
    setText('storage-map-indexed', this.scan.indexed_items.toLocaleString());
    setText('storage-map-time', formatDuration(this.scan.duration_ms));
    const indexStatus = document.getElementById('storage-map-index-status') as HTMLElement;
    indexStatus.textContent = this.scan.scan_mode;
    indexStatus.classList.toggle('fast', this.scan.scan_mode !== 'Parallel index');
    indexStatus.title = `${this.scan.indexed_items.toLocaleString()} file and folder records are searchable without rescanning`;
    setText('storage-map-skipped', this.scan.skipped ? `${this.scan.skipped.toLocaleString()} inaccessible entries skipped` : 'Complete scan');
    this.upButton.disabled = samePath(this.scan.current, this.scan.root);
    this.renderTreemap();
    this.renderList();
    this.updateSelection();
  }

  private renderTreemap(): void {
    const width = Math.max(1, Math.round(this.treemap.clientWidth));
    const height = Math.max(1, Math.round(this.treemap.clientHeight));
    const scale = Math.max(1, window.devicePixelRatio || 1);
    this.treemap.width = Math.round(width * scale);
    this.treemap.height = Math.round(height * scale);
    const context = this.treemap.getContext('2d');
    if (!context) return;
    context.setTransform(scale, 0, 0, scale, 0, 0);
    context.clearRect(0, 0, width, height);
    context.fillStyle = '#090c14';
    context.fillRect(0, 0, width, height);
    this.treemapRects = [];
    this.legend.replaceChildren();
    if (!this.scan) return;

    const source = (this.activeMapView === 'folders' ? this.scan.children : this.scan.largest_files)
      .filter((item) => item.bytes > 0)
      .sort((left, right) => right.bytes - left.bytes);
    if (!source.length) {
      drawCenteredText(context, width, height, 'No measurable files in this folder');
      this.visualMeta.textContent = 'No measurable items in this folder';
      return;
    }

    const visible = source.slice(0, 180);
    const measuredBytes = visible.reduce((total, item) => total + item.bytes, 0);
    const sourceBytes = source.reduce((total, item) => total + item.bytes, 0);
    const otherBytes = Math.max(0, this.scan.total_bytes - measuredBytes);
    const layoutItems: Array<{ item: StorageItem | null; label: string; bytes: number }> = visible.map((item) => ({
      item,
      label: item.name,
      bytes: item.bytes,
    }));
    if (otherBytes > 0) layoutItems.push({ item: null, label: 'Other measured items', bytes: otherBytes });

    this.treemapRects = squarifiedTreemap(layoutItems, 0, 0, 100, 100);
    const toneTotals = new Map<string, { tone: FileTone; bytes: number }>();
    for (const rect of this.treemapRects) {
      const tone = rect.item
        ? rect.item.is_directory ? folderTone(rect.item.name) : fileTone(rect.item.name)
        : OTHER_FILES_TONE;
      const current = toneTotals.get(tone.key);
      toneTotals.set(tone.key, { tone, bytes: (current?.bytes ?? 0) + rect.bytes });
      const x = (rect.x / 100) * width;
      const y = (rect.y / 100) * height;
      const rectWidth = (rect.width / 100) * width;
      const rectHeight = (rect.height / 100) * height;
      context.fillStyle = tone.color;
      context.fillRect(x + 0.5, y + 0.5, Math.max(0, rectWidth - 1), Math.max(0, rectHeight - 1));
      context.strokeStyle = rect.item && this.selected.has(rect.item.id) ? '#34d399' : 'rgba(5, 8, 14, 0.9)';
      context.lineWidth = rect.item && this.selected.has(rect.item.id) ? 2 : 1;
      context.strokeRect(x + 0.5, y + 0.5, Math.max(0, rectWidth - 1), Math.max(0, rectHeight - 1));

      if (rectWidth >= 62 && rectHeight >= 28) {
        context.save();
        context.beginPath();
        context.rect(x + 4, y + 3, Math.max(0, rectWidth - 8), Math.max(0, rectHeight - 6));
        context.clip();
        context.fillStyle = 'rgba(255, 255, 255, 0.93)';
        context.font = '600 10px Inter, sans-serif';
        context.fillText(rect.label, x + 6, y + 14);
        if (rectHeight >= 43) {
          context.fillStyle = 'rgba(255, 255, 255, 0.7)';
          context.font = '9px JetBrains Mono, monospace';
          context.fillText(formatBytes(rect.bytes), x + 6, y + 28);
          if (rectHeight >= 58 && this.scan.total_bytes > 0) {
            context.fillStyle = 'rgba(255, 255, 255, 0.58)';
            context.font = '8px JetBrains Mono, monospace';
            context.fillText(`${formatPercent(rect.bytes / this.scan.total_bytes)} of this folder`, x + 6, y + 41);
          }
        }
        context.restore();
      }
    }

    this.visualMeta.textContent = this.activeMapView === 'folders'
      ? `${source.length.toLocaleString()} direct items · open a folder block to drill down`
      : `${visible.length.toLocaleString()} largest files${sourceBytes < this.scan.total_bytes ? ' · remaining space grouped' : ''} · click to locate`;
    const legendTones = [...toneTotals.values()]
      .sort((left, right) => right.bytes - left.bytes);
    if (this.activeMapView === 'folders' && legendTones.some(({ tone }) => tone.key.startsWith('folder-'))) {
      const folders = element('span', 'storage-legend-item');
      const swatch = element('i', 'storage-folder-swatch');
      folders.append(swatch, document.createTextNode('Folders'));
      this.legend.appendChild(folders);
    }
    legendTones
      .filter(({ tone }) => this.activeMapView !== 'folders' || !tone.key.startsWith('folder-'))
      .forEach(({ tone }) => {
        const item = element('span', 'storage-legend-item');
        const swatch = element('i', '');
        swatch.style.background = tone.color;
        item.append(swatch, document.createTextNode(tone.label));
        this.legend.appendChild(item);
      });
  }

  private renderList(): void {
    this.list.replaceChildren();
    if (!this.scan) return;
    const query = this.searchInput.value.trim();
    if (query && this.searchResult?.query !== query) {
      this.list.appendChild(element('div', 'storage-empty-results', 'Searching the complete native index…'));
      return;
    }
    const source = query
      ? this.searchResult?.items ?? []
      : this.activeTab === 'children' ? this.scan.children : this.scan.largest_files;
    const items = source;
    if (!items.length) {
      this.list.appendChild(element('div', 'storage-empty-results', query ? 'No indexed file or folder matches this search' : 'No items to display'));
      return;
    }
    const visible = items.slice(0, 250);
    visible.forEach((item) => this.list.appendChild(this.itemRow(item)));
    const totalMatches = query ? this.searchResult?.total_matches ?? items.length : items.length;
    if (totalMatches > visible.length) {
      this.list.appendChild(element(
        'div',
        'storage-list-limit',
        `Showing ${visible.length.toLocaleString()} of ${totalMatches.toLocaleString()} indexed matches · refine the search for another file`,
      ));
    }
  }

  private scheduleSearch(): void {
    if (this.searchTimer) clearTimeout(this.searchTimer);
    const query = this.searchInput.value.trim();
    const requestId = ++this.searchRequestId;
    if (!query) {
      this.searchResult = null;
      this.renderList();
      this.renderTreemap();
      return;
    }
    this.searchResult = null;
    this.renderList();
    this.searchTimer = setTimeout(() => void this.runSearch(query, requestId), 120);
  }

  private async runSearch(query: string, requestId: number): Promise<void> {
    try {
      const result = await api.searchStorage(query);
      if (requestId !== this.searchRequestId || this.searchInput.value.trim() !== query) return;
      this.searchResult = result;
      this.renderList();
      this.visualMeta.textContent = `${result.total_matches.toLocaleString()} matches across ${result.indexed_items.toLocaleString()} indexed items · ${formatDuration(result.duration_ms)}`;
    } catch (error) {
      if (requestId !== this.searchRequestId) return;
      this.list.replaceChildren(element('div', 'storage-empty-results', `Index search stopped: ${String(error)}`));
    }
  }

  private treemapRectAt(event: PointerEvent | MouseEvent): TreemapRect | null {
    const bounds = this.treemap.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return null;
    const x = ((event.clientX - bounds.left) / bounds.width) * 100;
    const y = ((event.clientY - bounds.top) / bounds.height) * 100;
    return this.treemapRects.find((rect) => (
      x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height
    )) ?? null;
  }

  private describeTreemapPoint(event: PointerEvent): void {
    const rect = this.treemapRectAt(event);
    this.treemap.style.cursor = rect?.item ? 'pointer' : 'default';
    this.treemap.title = rect
      ? `${rect.item?.relative_path ?? rect.label}\n${formatBytes(rect.bytes)} · ${formatPercent(rect.bytes / Math.max(1, this.scan?.total_bytes ?? 1))}${rect.item?.is_directory ? '\nClick to open this folder' : rect.item ? '\nClick to locate this file' : ''}`
      : '';
  }

  private itemRow(item: StorageItem): HTMLElement {
    const row = element('div', `storage-row${this.selected.has(item.id) ? ' selected' : ''}`);
    row.dataset.storageId = item.id;
    const label = element('label', 'storage-row-check');
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.checked = this.selected.has(item.id);
    checkbox.setAttribute('aria-label', `Select ${item.name}`);
    checkbox.addEventListener('change', () => {
      if (checkbox.checked) this.selected.add(item.id);
      else this.selected.delete(item.id);
      row.classList.toggle('selected', checkbox.checked);
      this.updateTreemapSelection();
      this.updateSelection();
    });
    label.append(checkbox, element('span', 'storage-row-checkbox'));

    const name = element('span', 'storage-row-name');
    name.append(
      element('span', `storage-kind-icon ${item.is_directory ? 'folder' : 'file'}`, item.is_directory ? '▰' : '▪'),
      element('span', 'storage-name-copy'),
    );
    const copy = name.querySelector('.storage-name-copy') as HTMLElement;
    copy.append(element('strong', '', item.name), element('small', '', item.relative_path));
    const kind = item.is_directory
      ? `${item.files.toLocaleString()} files`
      : fileExtension(item.name);
    const size = element('span', 'storage-row-size');
    size.style.setProperty('--storage-share', `${Math.min(100, item.bytes / Math.max(1, this.scan?.total_bytes ?? 1) * 100)}%`);
    size.append(element('i', 'storage-size-bar'), element('strong', '', formatBytes(item.bytes)));
    row.append(
      label,
      name,
      element('span', 'storage-row-type', kind),
      element('span', 'storage-row-modified', formatDate(item.modified_ms)),
      size,
    );
    const open = element('button', 'storage-row-open') as HTMLButtonElement;
    open.type = 'button';
    open.textContent = item.is_directory ? '›' : '';
    open.disabled = !item.is_directory;
    open.title = item.is_directory ? `Open ${item.name}` : '';
    open.setAttribute('aria-label', item.is_directory ? `Open ${item.name}` : `${item.name} is a file`);
    if (item.is_directory) open.addEventListener('click', () => void this.openFolder(item));
    row.appendChild(open);
    return row;
  }

  private setTab(tab: StorageTab): void {
    this.activeTab = tab;
    document.querySelectorAll<HTMLButtonElement>('[data-storage-tab]').forEach((button) => {
      const active = button.dataset.storageTab === tab;
      button.classList.toggle('active', active);
      button.setAttribute('aria-selected', String(active));
    });
    this.renderList();
  }

  private setMapView(view: StorageMapView): void {
    this.activeMapView = view;
    document.querySelectorAll<HTMLButtonElement>('[data-storage-map-view]').forEach((button) => {
      const active = button.dataset.storageMapView === view;
      button.classList.toggle('active', active);
      button.setAttribute('aria-pressed', String(active));
    });
    this.renderTreemap();
  }

  private revealItem(item: StorageItem, tab: StorageTab = 'children'): void {
    this.setTab(tab);
    const source = tab === 'children' ? this.scan?.children ?? [] : this.scan?.largest_files ?? [];
    // The list intentionally renders at most 250 rows. If a canvas block is
    // farther down the result set, filter directly to it instead of creating a
    // large DOM list and then failing to reveal the clicked file.
    this.searchInput.value = source.indexOf(item) >= 250 ? item.relative_path : '';
    if (this.searchInput.value) this.scheduleSearch();
    else this.renderList();
    const row = this.list.querySelector<HTMLElement>(`[data-storage-id="${CSS.escape(item.id)}"]`);
    row?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    row?.classList.add('revealed');
    setTimeout(() => row?.classList.remove('revealed'), 850);
  }

  private async openFolder(item: StorageItem): Promise<void> {
    if (!item.is_directory) return;
    await this.navigateIndex(() => api.browseStorageItem(item.id));
  }

  private selectedItems(): StorageItem[] {
    if (!this.scan) return [];
    const unique = new Map<string, StorageItem>();
    [...this.scan.children, ...this.scan.largest_files, ...(this.searchResult?.items ?? [])].forEach((item) => {
      if (this.selected.has(item.id)) unique.set(item.id, item);
    });
    const items = [...unique.values()].sort((left, right) => pathDepth(left.relative_path) - pathDepth(right.relative_path));
    return items.filter((item, index) => !items.slice(0, index).some((parent) => isDescendant(item.relative_path, parent.relative_path)));
  }

  private updateSelection(): void {
    const selected = this.selectedItems();
    const bytes = selected.reduce((total, item) => total + item.bytes, 0);
    this.selectionText.textContent = selected.length
      ? `${selected.length} selected · ${formatBytes(bytes)}`
      : 'Nothing selected';
    this.recycleButton.disabled = this.busy || selected.length === 0;
  }

  private updateTreemapSelection(): void {
    this.renderTreemap();
  }

  private async recycleSelected(): Promise<void> {
    if (this.busy) return;
    const selected = this.selectedItems();
    if (!selected.length) return;
    const bytes = selected.reduce((total, item) => total + item.bytes, 0);
    const folders = selected.filter((item) => item.is_directory).length;
    const confirmed = await confirmDialog(
      `Move ${selected.length} selected ${selected.length === 1 ? 'item' : 'items'} (${formatBytes(bytes)}) to the Windows Recycle Bin?${folders ? ` This includes ${folders} ${folders === 1 ? 'folder' : 'folders'} and everything inside.` : ''}`,
      { title: 'Recycle selected items?', confirmText: 'Move to Recycle Bin', danger: true },
    );
    if (!confirmed) return;

    this.setBusy(true, 'Moving selected items to Recycle Bin…', 'recycle', this.scan?.current ?? 'Selected folder');
    try {
      const result = await api.recycleStorageItems(selected.map((item) => item.id));
      this.applyScan(result.scan);
      this.notify(`Moved ${result.items_recycled} ${result.items_recycled === 1 ? 'item' : 'items'} · ${formatBytes(result.bytes_recycled)} to Recycle Bin`);
    } catch (error) {
      this.notify(`Recycle operation stopped: ${String(error)}`);
    } finally {
      this.setBusy(false);
    }
  }

  private setBusy(
    busy: boolean,
    message = '',
    phase: StorageBusyPhase = 'scan',
    path = '',
  ): void {
    this.busy = busy;
    this.busyPhase = phase;
    this.recycling = phase === 'recycle';
    this.chooseButtons.forEach((button) => { button.disabled = busy; });
    this.rescanButton.disabled = busy;
    this.upButton.disabled = busy || !this.scan || samePath(this.scan.current, this.scan.root);
    this.searchInput.disabled = busy;
    this.closeButton.disabled = this.recycling;
    this.cancelButton.disabled = !busy || this.recycling || phase === 'picker';
    document.querySelectorAll<HTMLButtonElement>('[data-storage-tab]').forEach((button) => { button.disabled = busy; });
    this.progress.classList.toggle('hidden', !busy);
    this.progress.classList.toggle('picker', busy && phase === 'picker');
    this.progress.classList.toggle('recycling', busy && phase === 'recycle');
    this.progressTitle.textContent = message || 'Analyzing folder…';
    if (busy) {
      this.progressPath.textContent = phase === 'picker' ? 'Waiting for folder selection' : path;
      this.progressPath.title = phase === 'picker' ? '' : path;
      this.progressCount.textContent = phase === 'picker'
        ? 'Complete or cancel the Windows dialog'
        : phase === 'recycle'
          ? 'Windows is moving the selected items safely'
          : 'Starting native scanner…';
      this.progressElapsed.textContent = '0s elapsed';
      this.progressHelp.textContent = phase === 'picker'
        ? 'PicoBoost will begin measuring only after you choose a folder.'
        : phase === 'recycle'
          ? 'This step cannot be interrupted because Windows may already have moved some items.'
          : 'Large game folders can take a few minutes. Collapse this tool to keep using PicoBoost.';
      this.startProgressUpdates();
    } else {
      this.stopProgressUpdates();
    }
    this.subtitle.textContent = busy
      ? `${message || 'Analyzing folder…'} · running in background`
      : 'Understand a folder visually and choose what to recycle';
    this.overlay.classList.toggle('scanning', busy);
    this.recycleLabel.textContent = this.recycling ? 'Moving…' : 'Move to Recycle Bin';
    this.updateSelection();
  }

  private startProgressUpdates(): void {
    this.stopProgressUpdates();
    const cycle = ++this.progressCycle;
    this.progressStartedAt = performance.now();
    const update = (): void => {
      if (!this.busy) return;
      const elapsed = Math.max(0, performance.now() - this.progressStartedAt);
      this.progressElapsed.textContent = formatElapsed(elapsed);
      if (this.busyPhase === 'scan') void this.pollScanProgress(cycle);
    };
    update();
    this.progressTimer = setInterval(update, 250);
  }

  private stopProgressUpdates(): void {
    this.progressCycle += 1;
    if (this.progressTimer) clearInterval(this.progressTimer);
    this.progressTimer = null;
    this.progressPollActive = false;
  }

  private async pollScanProgress(cycle: number): Promise<void> {
    if (cycle !== this.progressCycle || this.progressPollActive || !this.busy || this.busyPhase !== 'scan') return;
    this.progressPollActive = true;
    try {
      const progress = await api.getStorageScanProgress();
      if (cycle !== this.progressCycle || !this.busy || this.busyPhase !== 'scan') return;
      if (progress.items_checked > 0) {
        const workers = progress.workers > 1 ? ` · ${progress.workers} scan workers` : '';
        this.progressCount.textContent = `${progress.items_checked.toLocaleString()} filesystem items checked${workers}`;
      } else if (this.progressTitle.textContent?.includes('NTFS')) {
        this.progressCount.textContent = 'Reading the read-only NTFS record index…';
      }
      if (progress.running && progress.elapsed_ms > 0) {
        this.progressElapsed.textContent = formatElapsed(progress.elapsed_ms);
      }
    } catch {
      // The local elapsed clock and activity bar remain useful if one progress
      // poll races with scan startup or shutdown.
    } finally {
      if (cycle === this.progressCycle) this.progressPollActive = false;
    }
  }
}

function squarifiedTreemap(
  items: Array<{ item: StorageItem | null; label: string; bytes: number }>,
  x: number,
  y: number,
  width: number,
  height: number,
): TreemapRect[] {
  if (!items.length) return [];
  const total = items.reduce((sum, item) => sum + Math.max(1, item.bytes), 0);
  const scale = width * height / total;
  const remaining = items
    .map((item) => ({ ...item, area: Math.max(1, item.bytes) * scale }))
    .sort((left, right) => right.area - left.area);
  const rectangles: TreemapRect[] = [];
  let availableX = x;
  let availableY = y;
  let availableWidth = width;
  let availableHeight = height;

  while (remaining.length && availableWidth > 0 && availableHeight > 0) {
    const shortSide = Math.min(availableWidth, availableHeight);
    const row = [remaining.shift()!];
    while (remaining.length) {
      const currentScore = treemapWorstRatio(row, shortSide);
      const nextScore = treemapWorstRatio([...row, remaining[0]], shortSide);
      if (nextScore > currentScore) break;
      row.push(remaining.shift()!);
    }
    const rowArea = row.reduce((sum, item) => sum + item.area, 0);
    if (availableWidth >= availableHeight) {
      const rowWidth = rowArea / Math.max(availableHeight, Number.EPSILON);
      let rowY = availableY;
      for (const item of row) {
        const itemHeight = item.area / Math.max(rowWidth, Number.EPSILON);
        rectangles.push({ ...item, x: availableX, y: rowY, width: rowWidth, height: itemHeight });
        rowY += itemHeight;
      }
      availableX += rowWidth;
      availableWidth = Math.max(0, availableWidth - rowWidth);
    } else {
      const rowHeight = rowArea / Math.max(availableWidth, Number.EPSILON);
      let rowX = availableX;
      for (const item of row) {
        const itemWidth = item.area / Math.max(rowHeight, Number.EPSILON);
        rectangles.push({ ...item, x: rowX, y: availableY, width: itemWidth, height: rowHeight });
        rowX += itemWidth;
      }
      availableY += rowHeight;
      availableHeight = Math.max(0, availableHeight - rowHeight);
    }
  }
  return rectangles;
}

function treemapWorstRatio(row: Array<{ area: number }>, shortSide: number): number {
  const total = row.reduce((sum, item) => sum + item.area, 0);
  const minimum = Math.min(...row.map((item) => item.area));
  const maximum = Math.max(...row.map((item) => item.area));
  const sideSquared = shortSide * shortSide;
  return Math.max(
    sideSquared * maximum / (total * total),
    total * total / (sideSquared * minimum),
  );
}

const FILE_TONES: Array<FileTone & { extensions: Set<string> }> = [
  { key: 'video', label: 'Video', color: '#7c3aed', extensions: new Set(['mp4', 'mkv', 'avi', 'mov', 'wmv', 'webm', 'm4v']) },
  { key: 'games', label: 'Games & apps', color: '#2563eb', extensions: new Set(['exe', 'dll', 'pak', 'vpk', 'bin', 'dat', 'bundle', 'asset', 'assets', 'obb']) },
  { key: 'archives', label: 'Archives', color: '#d97706', extensions: new Set(['zip', '7z', 'rar', 'iso', 'cab', 'gz', 'tar', 'xz']) },
  { key: 'images', label: 'Images', color: '#db2777', extensions: new Set(['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp', 'tif', 'tiff', 'psd', 'dds']) },
  { key: 'audio', label: 'Audio', color: '#059669', extensions: new Set(['wav', 'mp3', 'flac', 'ogg', 'm4a', 'aac', 'wma']) },
  { key: 'documents', label: 'Documents', color: '#0891b2', extensions: new Set(['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'txt', 'log', 'json', 'xml']) },
  { key: 'system', label: 'System & data', color: '#475569', extensions: new Set(['sys', 'db', 'sqlite', 'tmp', 'cache', 'msi', 'msp']) },
];

const DEFAULT_FILE_TONE: FileTone = { key: 'other', label: 'Other files', color: '#4f46e5' };
const OTHER_FILES_TONE: FileTone = { key: 'grouped', label: 'Grouped smaller files', color: '#334155' };
const FOLDER_TONES: FileTone[] = [
  { key: 'folder-blue', label: 'Folders', color: '#2563eb' },
  { key: 'folder-violet', label: 'Folders', color: '#6d4dd8' },
  { key: 'folder-cyan', label: 'Folders', color: '#0e7490' },
  { key: 'folder-teal', label: 'Folders', color: '#0f766e' },
  { key: 'folder-amber', label: 'Folders', color: '#a16207' },
];

function fileTone(name: string): FileTone {
  const dot = name.lastIndexOf('.');
  const extension = dot >= 0 ? name.slice(dot + 1).toLocaleLowerCase() : '';
  return FILE_TONES.find((tone) => tone.extensions.has(extension)) ?? DEFAULT_FILE_TONE;
}

function folderTone(name: string): FileTone {
  let hash = 0;
  for (let index = 0; index < name.length; index += 1) hash = (hash * 31 + name.charCodeAt(index)) >>> 0;
  return FOLDER_TONES[hash % FOLDER_TONES.length];
}

function drawCenteredText(context: CanvasRenderingContext2D, width: number, height: number, message: string): void {
  context.fillStyle = '#64748b';
  context.font = '10px Inter, sans-serif';
  context.textAlign = 'center';
  context.textBaseline = 'middle';
  context.fillText(message, width / 2, height / 2);
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

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`;
}

function formatDuration(milliseconds: number): string {
  if (milliseconds < 1000) return `${Math.max(0, Math.round(milliseconds))}ms`;
  return `${(milliseconds / 1000).toFixed(milliseconds < 10_000 ? 1 : 0)}s`;
}

function formatPercent(ratio: number): string {
  const percent = Math.max(0, ratio * 100);
  if (percent >= 10) return `${percent.toFixed(0)}%`;
  if (percent >= 1) return `${percent.toFixed(1)}%`;
  return `${percent.toFixed(2)}%`;
}

function formatElapsed(milliseconds: number): string {
  const seconds = Math.max(0, milliseconds / 1000);
  if (seconds < 10) return `${seconds.toFixed(1)}s elapsed`;
  if (seconds < 60) return `${Math.floor(seconds)}s elapsed`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m ${Math.floor(seconds % 60)}s elapsed`;
}

function nextPaint(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => setTimeout(resolve, 0));
  });
}

function formatDate(milliseconds: number | null): string {
  if (milliseconds === null) return 'Unknown';
  const date = new Date(milliseconds);
  return Number.isNaN(date.getTime()) ? 'Unknown' : date.toLocaleDateString([], { year: 'numeric', month: 'short', day: 'numeric' });
}

function fileExtension(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toUpperCase() : 'FILE';
}

function samePath(left: string, right: string): boolean {
  return left.replace(/[\\/]+$/, '').toLocaleLowerCase() === right.replace(/[\\/]+$/, '').toLocaleLowerCase();
}

function pathDepth(path: string): number {
  return path.split(/[\\/]/).filter(Boolean).length;
}

function isDescendant(path: string, parent: string): boolean {
  const normalizedPath = path.replace(/\//g, '\\').toLocaleLowerCase();
  const normalizedParent = parent.replace(/\//g, '\\').replace(/\\+$/, '').toLocaleLowerCase();
  return normalizedPath.startsWith(`${normalizedParent}\\`);
}
