// Styled, promise-based replacement for the browser's blocking confirm().
// A frameless window shouldn't surface native OS dialogs, so confirmations
// route through here for a consistent look (ported from PicoNote).

interface ConfirmOptions {
  title?: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
}

/** Confirmation dialog. Resolves true on OK/Enter, false on Cancel/Escape/backdrop. */
export function confirmDialog(message: string, opts: ConfirmOptions = {}): Promise<boolean> {
  return new Promise((resolve) => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialogId = `confirm-${crypto.randomUUID()}`;
    const overlay = document.createElement('div');
    overlay.className = 'confirm-overlay';
    const dialog = document.createElement('div');
    dialog.className = 'confirm-dialog';
    dialog.setAttribute('role', 'dialog');
    dialog.setAttribute('aria-modal', 'true');
    dialog.setAttribute('aria-labelledby', `${dialogId}-title`);
    dialog.setAttribute('aria-describedby', `${dialogId}-message`);

    const title = document.createElement('div');
    title.id = `${dialogId}-title`;
    title.className = 'confirm-title';
    title.textContent = opts.title || 'Confirm';
    const body = document.createElement('div');
    body.id = `${dialogId}-message`;
    body.className = 'confirm-message';
    body.textContent = message;
    const actions = document.createElement('div');
    actions.className = 'confirm-actions';
    const cancelBtn = document.createElement('button');
    cancelBtn.type = 'button';
    cancelBtn.className = 'confirm-btn cancel';
    const okBtn = document.createElement('button');
    okBtn.type = 'button';
    okBtn.className = `confirm-btn ok${opts.danger ? ' danger' : ''}`;
    cancelBtn.textContent = opts.cancelText || 'Cancel';
    okBtn.textContent = opts.confirmText || 'OK';
    actions.append(cancelBtn, okBtn);
    dialog.append(title, body, actions);
    overlay.appendChild(dialog);

    document.body.appendChild(overlay);
    okBtn.focus();

    let settled = false;
    const cleanup = (result: boolean) => {
      if (settled) return;
      settled = true;
      overlay.remove();
      document.removeEventListener('keydown', onKey, true);
      previousFocus?.focus();
      resolve(result);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopImmediatePropagation();
        cleanup(false);
      } else if (e.key === 'Enter') {
        e.preventDefault();
        e.stopImmediatePropagation();
        cleanup(document.activeElement !== cancelBtn);
      } else if (e.key === 'Tab') {
        const backwards = e.shiftKey;
        if ((!backwards && document.activeElement === okBtn) || (backwards && document.activeElement === cancelBtn)) {
          e.preventDefault();
          (backwards ? okBtn : cancelBtn).focus();
        }
      }
    };
    document.addEventListener('keydown', onKey, true);
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) cleanup(false); });
    cancelBtn.addEventListener('click', () => cleanup(false));
    okBtn.addEventListener('click', () => cleanup(true));
  });
}
