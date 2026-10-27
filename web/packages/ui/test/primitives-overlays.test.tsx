// CF1 Task 5: T3 Code's overlays under the Loams wrappers (D1073, D1077, D1081).

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import {
  Combobox,
  DIALOG_POPUP_CLASS,
  Dialog,
  MenuPopup,
  MenuRadioGroup,
  MenuRadioItem,
  MenuRoot,
  MenuTrigger,
  PopoverPopup,
  PopoverRoot,
  PopoverTrigger,
  PreviewCard,
  PreviewCardPopup,
  PreviewCardTrigger,
  SelectMenu,
  Sheet,
  Tooltip,
  TooltipPopup,
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
} from '../src';

afterEach(cleanup);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.getAnimations ??= () => [];

/** Base UI picks a list item on the pointer sequence, not on a bare click (T4-3). */
async function pick(el: HTMLElement) {
  await act(async () => {});
  fireEvent.pointerDown(el, { pointerType: 'mouse' });
  fireEvent.pointerUp(el, { pointerType: 'mouse' });
  fireEvent.mouseUp(el);
  fireEvent.click(el);
  await act(async () => {});
}

describe('dialog glass (D1073)', () => {
  it('a dialog popup is glass with the xl radius and the no-drag region', async () => {
    render(
      <Dialog open onClose={() => {}} title="Rename">
        body
      </Dialog>,
    );
    await act(async () => {});
    const dialog = screen.getByRole('dialog', { name: 'Rename' });
    expect(dialog.className).toContain('loams-dialog-glass');
    expect(dialog.className).toContain('rounded-xl');
    expect(DIALOG_POPUP_CLASS).toContain('[-webkit-app-region:no-drag]');
  });

  it('Escape closes the topmost overlay only (D1081)', async () => {
    const closed: string[] = [];
    render(
      <Dialog open onClose={() => closed.push('outer')} title="Outer">
        <Dialog open onClose={() => closed.push('inner')} title="Inner">
          inner
        </Dialog>
      </Dialog>,
    );
    await act(async () => {});
    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
    expect(closed).toEqual(['inner']);
  });

  it('a sheet snaps unless motion is on (D1076)', async () => {
    render(
      <Sheet open onClose={() => {}} title="Projects" side="left">
        list
      </Sheet>,
    );
    await act(async () => {});
    const sheet = screen.getByRole('dialog', { name: 'Projects' });
    expect(sheet.className).toContain('duration-(--op-panel-duration)');
    expect(sheet.className).toContain('motion-reduce:transition-none');
  });
});

describe('vendored parts', () => {
  it('a radio menu reports the chosen item', async () => {
    const chosen: string[] = [];
    render(
      <MenuRoot>
        <MenuTrigger>Sort</MenuTrigger>
        <MenuPopup>
          <MenuRadioGroup value="name" onValueChange={(v) => chosen.push(String(v))}>
            <MenuRadioItem value="name">Name</MenuRadioItem>
            <MenuRadioItem value="date">Date</MenuRadioItem>
          </MenuRadioGroup>
        </MenuPopup>
      </MenuRoot>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Sort' }));
    fireEvent.click(await screen.findByRole('menuitemradio', { name: 'Date' }));
    expect(chosen).toEqual(['date']);
  });

  it('a popover opens from its trigger, and a tooltip names its trigger', async () => {
    render(
      <TooltipProvider>
        <PopoverRoot>
          <PopoverTrigger>Filters</PopoverTrigger>
          <PopoverPopup>filter body</PopoverPopup>
        </PopoverRoot>
        <TooltipRoot open>
          <TooltipTrigger render={<button type="button">i</button>} />
          <TooltipPopup>Inspector (⌘I)</TooltipPopup>
        </TooltipRoot>
        <PreviewCard open>
          <PreviewCardTrigger href="#/data">orders</PreviewCardTrigger>
          <PreviewCardPopup>12 columns</PreviewCardPopup>
        </PreviewCard>
      </TooltipProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Filters' }));
    expect(await screen.findByText('filter body')).toBeTruthy();
    expect(await screen.findByText('Inspector (⌘I)')).toBeTruthy();
    expect(await screen.findByText('12 columns')).toBeTruthy();
  });

  it('the Loams Tooltip wraps its child as the trigger', async () => {
    render(
      <TooltipProvider>
        <Tooltip content="Copy the id">
          <button type="button">Copy</button>
        </Tooltip>
      </TooltipProvider>,
    );
    const trigger = screen.getByRole('button', { name: 'Copy' });
    expect(trigger.getAttribute('data-slot')).toBe('tooltip-trigger');
  });

  it('SelectMenu keeps its props on the new parts', async () => {
    const picked: string[] = [];
    render(
      <SelectMenu
        label="Region"
        value="ams"
        onValueChange={(v) => picked.push(v)}
        options={[
          { value: 'ams', label: 'Amsterdam' },
          { value: 'ber', label: 'Berlin' },
        ]}
      />,
    );
    const trigger = screen.getByRole('combobox', { name: 'Region' });
    expect(trigger.getAttribute('data-slot')).toBe('select-trigger');
    fireEvent.click(trigger);
    await pick(await screen.findByRole('option', { name: 'Berlin' }));
    expect(picked).toEqual(['ber']);
  });

  it('Combobox keeps its props on the new parts', async () => {
    const picked: (string | null)[] = [];
    render(
      <Combobox
        label="Branch"
        value={null}
        onValueChange={(v) => picked.push(v)}
        options={['main', 'dev']}
      />,
    );
    const input = screen.getByRole('combobox', { name: 'Branch' });
    expect(input.closest('[data-slot="combobox-input"], [data-slot="input-control"]')).toBeTruthy();
    // jsdom: Base UI opens the list on a key, not on a synthetic change event.
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    await act(async () => {});
    fireEvent.change(input, { target: { value: 'de' } });
    await act(async () => {});
    expect(screen.queryByRole('option', { name: 'main' })).toBeNull();
    await pick(await screen.findByRole('option', { name: 'dev' }));
    expect(picked).toEqual(['dev']);
  });
});
