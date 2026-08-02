import { api } from './api';
import { LaunchApplication } from './types';

const STORAGE_KEY = 'picoboost_launch_applications_v1';
const MAX_CUSTOM_APPS = 20;

type Notify = (message: string) => void;
type Changed = (applications: LaunchApplication[]) => void;

const steamDefault = (): LaunchApplication => ({
  id: 'builtin-steam',
  name: 'Steam',
  path: null,
  enabled: true,
  builtIn: true,
});

export function loadLaunchApplications(): LaunchApplication[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [steamDefault()];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [steamDefault()];

    const custom: LaunchApplication[] = [];
    let steam = steamDefault();
    for (const value of parsed) {
      if (!value || typeof value !== 'object') continue;
      const item = value as Partial<LaunchApplication>;
      if (item.id === 'builtin-steam') {
        steam = { ...steamDefault(), enabled: item.enabled !== false };
        continue;
      }
      if (
        typeof item.path !== 'string' ||
        !item.path.toLowerCase().endsWith('.exe') ||
        typeof item.name !== 'string' ||
        !item.name.trim()
      ) continue;
      custom.push({
        id: typeof item.id === 'string' && item.id ? item.id : crypto.randomUUID(),
        name: normalizeName(item.name),
        path: item.path,
        enabled: item.enabled !== false,
        builtIn: false,
      });
      if (custom.length >= MAX_CUSTOM_APPS) break;
    }
    return [steam, ...deduplicate(custom)];
  } catch {
    return [steamDefault()];
  }
}

function saveLaunchApplications(applications: LaunchApplication[]): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(applications));
}

/** Dedicated settings page for the Launch pipeline capability. */
export class LaunchAppsModal {
  private readonly overlay = document.getElementById('launch-apps-overlay') as HTMLElement;
  private readonly content = document.getElementById('launch-apps-content') as HTMLElement;
  private readonly closeButton = document.getElementById('launch-apps-close') as HTMLButtonElement;
  private readonly addButton = document.getElementById('launch-apps-add') as HTMLButtonElement;
  private applications = loadLaunchApplications();
  private picking = false;
  private previousFocus: HTMLElement | null = null;

  constructor(private readonly notify: Notify, private readonly changed: Changed) {
    document.getElementById('launch-apps-configure')?.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
      this.open();
    });
    this.closeButton.addEventListener('click', () => this.close());
    this.addButton.addEventListener('click', () => void this.addApplications());
    this.overlay.addEventListener('mousedown', (event) => {
      if (event.target === this.overlay) this.close();
    });
    document.addEventListener('keydown', (event) => {
      if (
        event.key === 'Escape' &&
        !document.querySelector('.confirm-overlay') &&
        !this.overlay.classList.contains('hidden') &&
        !this.picking
      ) {
        event.preventDefault();
        this.close();
      }
    });
    this.changed(this.applications);
  }

  private open(): void {
    if (!this.overlay.classList.contains('hidden')) return;
    this.applications = loadLaunchApplications();
    this.previousFocus = document.activeElement as HTMLElement | null;
    this.render();
    this.overlay.classList.remove('hidden');
    this.overlay.setAttribute('aria-hidden', 'false');
    this.closeButton.focus();
  }

  private close(): void {
    if (this.picking) return;
    this.overlay.classList.add('hidden');
    this.overlay.setAttribute('aria-hidden', 'true');
    this.previousFocus?.focus();
  }

  private commit(): void {
    saveLaunchApplications(this.applications);
    this.changed(this.applications);
    this.render();
  }

  private async addApplications(): Promise<void> {
    if (this.picking) return;
    const customCount = this.applications.filter((application) => !application.builtIn).length;
    if (customCount >= MAX_CUSTOM_APPS) {
      this.notify(`The launch list supports up to ${MAX_CUSTOM_APPS} custom applications`);
      return;
    }
    this.picking = true;
    this.addButton.disabled = true;
    this.addButton.textContent = 'Choosing…';
    try {
      const paths = await api.pickLaunchApplications();
      const existing = new Set(
        this.applications
          .map((application) => application.path?.toLowerCase())
          .filter((path): path is string => Boolean(path)),
      );
      let added = 0;
      for (const path of paths) {
        if (existing.has(path.toLowerCase()) || customCount + added >= MAX_CUSTOM_APPS) continue;
        this.applications.push({
          id: crypto.randomUUID(),
          name: displayName(path),
          path,
          enabled: true,
          builtIn: false,
        });
        existing.add(path.toLowerCase());
        added += 1;
      }
      if (added) {
        this.commit();
        this.notify(`${added} ${added === 1 ? 'application' : 'applications'} added to Launch`);
      }
    } catch (error) {
      this.notify(`Application picker unavailable: ${String(error)}`);
    } finally {
      this.picking = false;
      this.addButton.disabled = false;
      this.addButton.textContent = 'Add applications';
    }
  }

  private render(): void {
    this.content.replaceChildren();
    const enabled = this.applications.filter((application) => application.enabled).length;
    const summary = node('div', 'launch-apps-summary');
    summary.append(
      node('strong', '', `${enabled} enabled`),
      node('span', '', 'Applications launch in this order at normal Windows priority.'),
    );
    this.content.appendChild(summary);

    const list = node('div', 'launch-apps-list');
    this.applications.forEach((application, index) => list.appendChild(this.applicationRow(application, index)));
    this.content.appendChild(list);
  }

  private applicationRow(application: LaunchApplication, index: number): HTMLElement {
    const row = node('article', `launch-app-row${application.enabled ? '' : ' disabled'}`);
    const checkboxLabel = node('label', 'launch-app-enabled');
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.checked = application.enabled;
    checkbox.setAttribute('aria-label', `Launch ${application.name}`);
    checkbox.addEventListener('change', () => {
      application.enabled = checkbox.checked;
      this.commit();
    });
    checkboxLabel.append(checkbox, node('span', 'launch-app-check'));

    const icon = node('span', `launch-app-icon${application.builtIn ? ' steam' : ''}`);
    icon.textContent = application.builtIn ? 'S' : application.name.slice(0, 1).toUpperCase();
    const copy = node('span', 'launch-app-copy');
    const nameRow = node('span', 'launch-app-name-row');
    nameRow.append(node('strong', '', application.name));
    if (application.builtIn) nameRow.append(node('span', 'launch-built-in', 'Default'));
    copy.append(
      nameRow,
      node('span', 'launch-app-path', application.path ?? 'Auto-detected from the Steam installation'),
    );

    const actions = node('span', 'launch-app-actions');
    if (!application.builtIn) {
      actions.append(
        this.actionButton('↑', 'Move earlier', index === 1, () => this.move(index, -1)),
        this.actionButton('↓', 'Move later', index === this.applications.length - 1, () => this.move(index, 1)),
        this.actionButton('×', `Remove ${application.name}`, false, () => this.remove(application.id), true),
      );
    }
    row.append(checkboxLabel, icon, copy, actions);
    return row;
  }

  private actionButton(label: string, title: string, disabled: boolean, action: () => void, danger = false): HTMLButtonElement {
    const button = node('button', `launch-row-action${danger ? ' danger' : ''}`, label) as HTMLButtonElement;
    button.type = 'button';
    button.title = title;
    button.setAttribute('aria-label', title);
    button.disabled = disabled;
    button.addEventListener('click', action);
    return button;
  }

  private move(index: number, offset: number): void {
    const target = index + offset;
    if (index <= 0 || target <= 0 || target >= this.applications.length) return;
    [this.applications[index], this.applications[target]] = [this.applications[target], this.applications[index]];
    this.commit();
  }

  private remove(id: string): void {
    this.applications = this.applications.filter((application) => application.builtIn || application.id !== id);
    this.commit();
  }
}

function deduplicate(applications: LaunchApplication[]): LaunchApplication[] {
  const paths = new Set<string>();
  return applications.filter((application) => {
    const path = application.path?.toLowerCase();
    if (!path || paths.has(path)) return false;
    paths.add(path);
    return true;
  });
}

function displayName(path: string): string {
  const filename = path.split(/[\\/]/).pop() ?? 'Application';
  return normalizeName(filename.replace(/\.exe$/i, '').replace(/[-_]+/g, ' '));
}

function normalizeName(name: string): string {
  return name.trim().replace(/\s+/g, ' ').slice(0, 80) || 'Application';
}

function node(tag: string, className = '', text = ''): HTMLElement {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}
