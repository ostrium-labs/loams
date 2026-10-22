import { Button, Dialog, Notice } from '@loams/ui';
import { useId, useState } from 'react';
import type { IssuedPassword } from './types.js';

/**
 * A role's password, shown once. The server returns it on `CreateRole` and
 * `ResetRolePassword` only; closing this dialog drops it from the page's state, and the
 * page never writes it to storage or a log.
 */
export function PasswordOnce({
  issued,
  onClose,
  copy,
}: {
  issued: (IssuedPassword & { reset: boolean }) | undefined;
  onClose: () => void;
  copy?: (text: string) => Promise<void> | void;
}) {
  const [copied, setCopied] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);
  const [shown, setShown] = useState(false);
  const fieldId = useId();
  const close = () => {
    setCopied(false);
    setCopyFailed(false);
    setShown(false);
    onClose();
  };
  return (
    <Dialog
      open={issued !== undefined}
      onClose={close}
      title={issued ? `Password for ${issued.role.name}` : ''}
      footer={
        <Button variant="primary" onClick={close}>
          I have saved it
        </Button>
      }
    >
      {issued && (
        <div className="flex flex-col gap-3">
          <Notice tone="warn" title="Shown once">
            {issued.reset ? 'The old password no longer works. ' : ''}
            Copy this password now. Loams does not show it again; reset it if it is lost.
          </Notice>
          <label htmlFor={fieldId} className="text-body">
            Password
          </label>
          <div className="flex gap-2">
            <input
              id={fieldId}
              className={/* mono-ok: id */ 'loams-input font-mono'}
              type={shown ? 'text' : 'password'}
              value={issued.password}
              readOnly
              autoComplete="off"
              spellCheck={false}
              onFocus={(e) => e.currentTarget.select()}
            />
            <Button size="sm" aria-pressed={shown} onClick={() => setShown((v) => !v)}>
              {shown ? 'Hide' : 'Show'}
            </Button>
            {copy && (
              <Button
                size="sm"
                onClick={async () => {
                  try {
                    await copy(issued.password);
                    setCopyFailed(false);
                    setCopied(true);
                  } catch {
                    setCopied(false);
                    setCopyFailed(true);
                  }
                }}
              >
                Copy
              </Button>
            )}
          </div>
          <p className="text-small text-muted" aria-live="polite">
            {copyFailed
              ? 'Could not copy. Show the password and copy it from the field.'
              : copied
                ? 'Copied to the clipboard.'
                : ''}
          </p>
        </div>
      )}
    </Dialog>
  );
}
