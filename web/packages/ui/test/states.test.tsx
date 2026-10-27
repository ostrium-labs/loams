// Component states (UI1): default, hover and pressed are CSS (styles.css, checked below by
// rule); focus, disabled, loading, selected and error are attributes a test can read.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { Button, IconButton, InputControl, ListRow, SearchInput, TooltipProvider } from '../src';

const STYLES = readFileSync(join(import.meta.dirname, '../src/styles.css'), 'utf8');
afterEach(cleanup);

describe('Button', () => {
  it('has hover, pressed, focus-visible and disabled rules for every variant', () => {
    for (const v of ['primary', 'secondary', 'quiet', 'danger', 'danger-solid', 'link']) {
      expect(STYLES, v).toMatch(new RegExp(`\\.loams-btn-${v}:hover`));
    }
    expect(STYLES).toMatch(
      /\.loams-btn:active:not\(:disabled, \[aria-disabled="true"\]\)\s*\{\s*transform: scale\(0\.98\)/,
    );
    expect(STYLES).toMatch(
      /\.loams-btn:focus-visible\s*\{\s*outline: var\(--op-focus-width\) solid var\(--op-focus\)/,
    );
    expect(STYLES).toMatch(/\.loams-btn:disabled,\s*\.loams-btn\[aria-disabled="true"\]/);
  });

  it('does not press under reduced motion', () => {
    const reduce = STYLES.slice(
      STYLES.indexOf('@media (prefers-reduced-motion: reduce) {\n  .loams-btn'),
    );
    expect(reduce).toMatch(/transform: none/);
  });

  it('while loading: busy, keeps its name and focus, ignores presses', () => {
    let pressed = 0;
    const { rerender } = render(
      <Button variant="primary" loading onClick={() => pressed++}>
        Save
      </Button>,
    );
    const button = screen.getByRole('button', { name: 'Save' });
    expect(button.getAttribute('aria-busy')).toBe('true');
    expect(button.getAttribute('aria-disabled')).toBe('true');
    expect(button.hasAttribute('disabled')).toBe(false);
    expect(button.querySelector('[data-slot="spinner"]')).not.toBeNull();
    button.focus();
    fireEvent.click(button);
    expect(pressed).toBe(0);
    expect(document.activeElement).toBe(button);
    rerender(
      <Button variant="primary" onClick={() => pressed++}>
        Save
      </Button>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(pressed).toBe(1);
    expect(screen.getByRole('button').hasAttribute('aria-busy')).toBe(false);
    expect(screen.getByRole('button').querySelector('[data-slot="spinner"]')).toBeNull();
  });

  it('when disabled is a native disabled button', () => {
    render(<Button disabled>Delete</Button>);
    expect((screen.getByRole('button', { name: 'Delete' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});

describe('IconButton', () => {
  it('is named by its label and shows it as a tooltip trigger', () => {
    render(
      <TooltipProvider>
        <IconButton label="Refresh">
          <svg aria-hidden="true" />
        </IconButton>
      </TooltipProvider>,
    );
    const button = screen.getByRole('button', { name: 'Refresh' });
    expect(button.className).toContain('loams-btn-icon');
    expect(button.className).toContain('loams-btn-quiet');
  });

  it('can drop the tooltip, and swaps its icon for a spinner while loading', () => {
    render(
      <IconButton label="Sync" size="sm" tooltip={false} loading>
        <svg aria-hidden="true" data-testid="icon" />
      </IconButton>,
    );
    const button = screen.getByRole('button', { name: 'Sync' });
    expect(button.className).toContain('loams-btn-icon-sm');
    expect(screen.queryByTestId('icon')).toBeNull();
    expect(button.querySelector('[data-slot="spinner"]')).not.toBeNull();
  });
});

describe('InputControl', () => {
  it('shows its error and disabled states through the box', () => {
    render(<InputControl aria-label="Name" aria-invalid disabled />);
    const input = screen.getByRole('textbox', { name: 'Name' }) as HTMLInputElement;
    expect(input.disabled).toBe(true);
    const box = input.closest('[data-slot="input-control"]') as HTMLElement;
    for (const cls of [
      'has-aria-invalid:border-danger',
      'has-disabled:opacity-60',
      'hover:not-has-disabled:not-has-aria-invalid:border-muted',
      'has-focus-visible:outline-focus',
    ]) {
      expect(box.className).toContain(cls);
    }
  });
});

describe('SearchInput', () => {
  function Search() {
    const [q, setQ] = useState('');
    return <SearchInput label="Search tables" value={q} onValueChange={setQ} />;
  }

  it('clears with the button and with Escape, and Escape stops only when it cleared', () => {
    let escapes = 0;
    render(
      // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- a test listener for bubbling keys
      <div onKeyDown={(e) => e.key === 'Escape' && escapes++}>
        <Search />
      </div>,
    );
    const input = screen.getByRole('searchbox', { name: 'Search tables' }) as HTMLInputElement;
    expect(screen.queryByRole('button', { name: 'Clear search' })).toBeNull();
    fireEvent.change(input, { target: { value: 'orders' } });
    fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    expect(input.value).toBe('');
    expect(document.activeElement).toBe(input);
    fireEvent.change(input, { target: { value: 'users' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(input.value).toBe('');
    expect(escapes).toBe(0);
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(escapes).toBe(1);
  });
});

describe('ListRow', () => {
  it('renders as the element it is, with selected and disabled states', () => {
    render(
      <nav>
        <ListRow render={<a href="#/data" aria-current="page" />} meta="12" selected>
          Data
        </ListRow>
        <ListRow render={<button type="button" />} disabled density="compact">
          Jobs
        </ListRow>
      </nav>,
    );
    const data = screen.getByRole('link', { name: /Data/ });
    expect(data.getAttribute('data-selected')).toBe('');
    expect(data.getAttribute('aria-current')).toBe('page');
    expect(data.querySelector('.loams-row-meta')?.textContent).toBe('12');
    const jobs = screen.getByRole('button', { name: 'Jobs' });
    expect(jobs.getAttribute('aria-disabled')).toBe('true');
    expect(jobs.getAttribute('data-density')).toBe('compact');
  });

  it('ignores a press while disabled, whatever onClick the caller passed', () => {
    const onClick = vi.fn();
    const outer = vi.fn();
    render(
      // oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions -- a bubble-phase listener the row must not reach
      <div onClick={outer}>
        <ListRow render={<button type="button" />} disabled onClick={onClick}>
          Jobs
        </ListRow>
        <ListRow render={<button type="button" />} onClick={onClick}>
          Data
        </ListRow>
      </div>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Jobs' }));
    expect(onClick).not.toHaveBeenCalled();
    expect(outer).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Data' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('has hover, selected, inset focus and disabled rules', () => {
    expect(STYLES).toMatch(/\.loams-row:hover:not\(\[data-disabled\]\)/);
    expect(STYLES).toMatch(
      /\.loams-row\[data-selected\],\s*\.loams-row\[aria-current="page"\],\s*\.loams-row\[aria-selected="true"\]/,
    );
    expect(STYLES).toMatch(
      /\.loams-row:focus-visible\s*\{[^}]*outline-offset: calc\(var\(--op-focus-width\) \* -1\)/,
    );
    expect(STYLES).toMatch(/\.loams-row\[data-disabled\]\s*\{[^}]*cursor: not-allowed/);
  });
});
