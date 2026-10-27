// The primitives Studio needs, adapted from T3 Code's components/ui (SD1 Task 16;
// docs/design/studio-vendor-inventory.md group (k); docs/design/console-ui-stack.md 1–2).

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { type ReactElement, useEffect, useState } from 'react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import {
  Alert,
  AlertDialog,
  AlertTitle,
  Badge,
  Button,
  CheckboxControl,
  Collapsible,
  CollapsiblePanel,
  CollapsibleTrigger,
  type CommandItem,
  CommandPalette,
  ConfirmDialog,
  Dialog,
  InputControl,
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
  Kbd,
  KbdGroup,
  Menu,
  MenuCheckboxItem,
  MenuGroupLabel,
  MenuItem,
  MenuPart,
  MenuPartSeparator,
  MenuPopup,
  MenuRadioGroup,
  MenuRadioItem,
  MenuRoot,
  MenuShortcut,
  MenuTrigger,
  MiddleTruncate,
  PopoverPopup,
  PopoverRoot,
  PopoverTitle,
  PopoverTrigger,
  PreviewCard,
  PreviewCardPopup,
  PreviewCardTrigger,
  ResizeHandle,
  ScrollArea,
  Separator,
  Sheet,
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  Skeleton,
  SkeletonList,
  Spinner,
  Switch,
  splitForMiddleTruncate,
  Toaster,
  ToastProvider,
  Toggle,
  ToggleGroup,
  TooltipPopup,
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
  TypedConfirmDialog,
  useToast,
} from '../src';

afterEach(cleanup);

// jsdom has no ResizeObserver or Element.getAnimations; Base UI's scroll area uses both.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.getAnimations ??= () => [];

/* renders_with_tokens_only ------------------------------------------------------------ */

/** A raw colour literal (the same pattern as colours.test.ts). */
const LITERAL =
  /(?<![\w&/-])#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{3,4})(?![\w-])|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(\s*[-\d.]/i;
/** Tailwind's default palette, cleared in the console's tailwind.css, and T3 Code's own names. */
const PALETTE =
  /^(?:[a-z-]+:)*(?:bg|text|border|ring|fill|stroke|outline|shadow|from|to|via|divide|accent|caret|decoration)-(?:(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d|white$|black$|foreground|background|primary|secondary|popover|sidebar|destructive|muted-foreground|placeholder|success|warning|card)/;
/** Corners come from the scale (D1073): none, xs, sm, md, lg, xl, pill, panel. */
const RADIUS = /^(?:[a-z-]+:)*rounded(?:-[a-z]+)?-(?:2xl|3xl|full)$|^rounded$/;

/** Every class in the document and every inline style that breaks the token rule. */
function offTokenClasses(root: ParentNode = document.body): string[] {
  const bad: string[] = [];
  for (const el of root.querySelectorAll('*')) {
    const cls = el.getAttribute('class') ?? '';
    for (const c of cls.split(/\s+/).filter(Boolean)) {
      if (LITERAL.test(c) || PALETTE.test(c) || RADIUS.test(c) || c.includes('[#')) bad.push(c);
    }
    const style = el.getAttribute('style') ?? '';
    if (LITERAL.test(style)) bad.push(style);
  }
  return bad;
}

const ITEMS: CommandItem[] = [
  { id: 'new', text: 'New thread', group: 'Threads', shortcut: ['Mod', 'N'] },
  { id: 'open', text: 'Open project', group: 'Projects' },
  { id: 'settings', text: 'Settings', group: 'Go to' },
];

/** Every toast type, an action, a secondary action and details, in one stack. */
function RaiseToasts() {
  const toast = useToast();
  useEffect(() => {
    for (const type of ['success', 'error', 'warning', 'info', 'loading'] as const) {
      toast.add({ title: `A ${type} toast`, description: 'Details', type });
    }
    toast.add({
      title: 'Table dropped',
      actionProps: { children: 'Undo' },
      data: { secondaryActionProps: { children: 'View' }, expandableContent: 'orders' },
    });
  }, [toast]);
  return null;
}

function ToastSpecimen() {
  return (
    <ToastProvider>
      <RaiseToasts />
      <Toaster />
    </ToastProvider>
  );
}

const SPECIMENS: [string, () => ReactElement][] = [
  [
    'CommandPalette',
    () => <CommandPalette open onOpenChange={() => {}} items={ITEMS} onSelect={() => {}} />,
  ],
  [
    'Sidebar',
    () => (
      <Sidebar label="Threads" width={280} onWidthChange={() => {}}>
        <SidebarHeader>Studio</SidebarHeader>
        <SidebarContent>
          <SidebarGroup label="Pinned">
            <SidebarMenu>
              <SidebarMenuItem>
                <SidebarMenuButton active>Fix the cache</SidebarMenuButton>
              </SidebarMenuItem>
            </SidebarMenu>
          </SidebarGroup>
        </SidebarContent>
        <SidebarFooter>Settings</SidebarFooter>
      </Sidebar>
    ),
  ],
  [
    'Kbd',
    () => (
      <KbdGroup>
        <Kbd>Ctrl</Kbd>
        <Kbd>K</Kbd>
      </KbdGroup>
    ),
  ],
  [
    'ToggleGroup',
    () => (
      <ToggleGroup label="View" value={['split']} onValueChange={() => {}}>
        <Toggle value="split">Split</Toggle>
        <Toggle value="unified">Unified</Toggle>
      </ToggleGroup>
    ),
  ],
  [
    'InputGroup',
    () => (
      <InputGroup>
        <InputGroupAddon>⌕</InputGroupAddon>
        <InputGroupInput aria-label="Search" />
      </InputGroup>
    ),
  ],
  ['MiddleTruncate', () => <MiddleTruncate value="fix/cache-main-20260918-180825" />],
  [
    'ScrollArea',
    () => (
      <ScrollArea label="Log">
        <p>line</p>
      </ScrollArea>
    ),
  ],
  ['Switch', () => <Switch label="Auto-approve" checked={false} onCheckedChange={() => {}} />],
  [
    'Collapsible',
    () => (
      <Collapsible defaultOpen>
        <CollapsibleTrigger>Details</CollapsibleTrigger>
        <CollapsiblePanel>Body</CollapsiblePanel>
      </Collapsible>
    ),
  ],
  ['Spinner', () => <Spinner />],
  ['Skeleton', () => <Skeleton className="h-4 w-24" shape="pill" />],
  [
    'AlertDialog',
    () => (
      <AlertDialog
        open
        onClose={() => {}}
        title="Delete thread?"
        confirm="Delete"
        tone="danger"
        onConfirm={() => {}}
      >
        This cannot be undone.
      </AlertDialog>
    ),
  ],
  [
    'Sheet',
    () => (
      <Sheet open onClose={() => {}} title="Projects" side="left">
        <p>list</p>
      </Sheet>
    ),
  ],
  ['Button', () => <Button variant="primary">x</Button>],
  ['Badge', () => <Badge variant="info">x</Badge>],
  ['InputControl', () => <InputControl aria-label="x" />],
  ['CheckboxControl', () => <CheckboxControl aria-label="x" />],
  [
    'Alert',
    () => (
      <Alert variant="danger">
        <AlertTitle>x</AlertTitle>
      </Alert>
    ),
  ],
  ['Separator', () => <Separator />],
  [
    'Dialog',
    () => (
      <Dialog open onClose={() => {}} title="t" footer={<Button>ok</Button>}>
        b
      </Dialog>
    ),
  ],
  [
    'PopoverPopup',
    () => (
      <PopoverRoot open>
        <PopoverTrigger>Filters</PopoverTrigger>
        <PopoverPopup>
          <PopoverTitle>Filters</PopoverTitle>
          body
        </PopoverPopup>
      </PopoverRoot>
    ),
  ],
  [
    'MenuPopup',
    () => (
      <MenuRoot open>
        <MenuTrigger>Sort</MenuTrigger>
        <MenuPopup>
          <MenuRadioGroup value="name">
            <MenuGroupLabel>Order</MenuGroupLabel>
            <MenuRadioItem value="name">Name</MenuRadioItem>
            <MenuRadioItem value="date">Date</MenuRadioItem>
          </MenuRadioGroup>
          <MenuPartSeparator />
          <MenuCheckboxItem checked>Archived</MenuCheckboxItem>
          <MenuCheckboxItem checked variant="switch">
            Live
          </MenuCheckboxItem>
          <MenuPart variant="danger">
            Delete <MenuShortcut>⌘⌫</MenuShortcut>
          </MenuPart>
        </MenuPopup>
      </MenuRoot>
    ),
  ],
  [
    'TooltipPopup',
    () => (
      <TooltipProvider>
        <TooltipRoot open>
          <TooltipTrigger render={<button type="button">i</button>} />
          <TooltipPopup>Inspector</TooltipPopup>
        </TooltipRoot>
      </TooltipProvider>
    ),
  ],
  [
    'PreviewCardPopup',
    () => (
      <PreviewCard open>
        <PreviewCardTrigger href="#/data">orders</PreviewCardTrigger>
        <PreviewCardPopup>12 columns</PreviewCardPopup>
      </PreviewCard>
    ),
  ],
  ['Toaster', () => <ToastSpecimen />],
  [
    'ConfirmDialog',
    () => (
      <ConfirmDialog
        open
        onClose={() => {}}
        onConfirm={() => {}}
        object="the branch fix-cache"
        impact="Its 3 commits are deleted."
        confirm="Delete branch"
      />
    ),
  ],
  [
    'TypedConfirmDialog',
    () => (
      <TypedConfirmDialog
        open
        onClose={() => {}}
        onConfirm={() => {}}
        object="the table orders on production"
        impact="Production data is deleted."
        phrase="production"
        confirm="Drop table"
      />
    ),
  ],
];

describe('renders_with_tokens_only', () => {
  it.each(SPECIMENS)('%s uses token classes only', async (_name, make) => {
    render(make());
    // Portalled pop-ups mount after a tick.
    await act(async () => {});
    expect(document.body.innerHTML.length).toBeGreaterThan(0);
    expect(offTokenClasses()).toEqual([]);
  });

  it('the scan catches what it is meant to', () => {
    const div = document.createElement('div');
    div.innerHTML =
      '<span class="bg-red-500 rounded-2xl text-foreground hover:bg-[#fff] rounded-full"></span>' +
      '<span class="bg-surface rounded-pill rounded-md border-input rounded-panel text-muted"></span>';
    expect(offTokenClasses(div)).toEqual([
      'bg-red-500',
      'rounded-2xl',
      'text-foreground',
      'hover:bg-[#fff]',
      'rounded-full',
    ]);
  });
});

/* command_arrow_keys_and_enter ------------------------------------------------------- */

describe('CommandPalette', () => {
  it('command_arrow_keys_and_enter', async () => {
    const picked: string[] = [];
    render(
      <CommandPalette
        open
        onOpenChange={() => {}}
        items={ITEMS}
        onSelect={(item) => picked.push(item.id)}
      />,
    );
    const dialog = await screen.findByRole('dialog', { name: 'Command palette' });
    const input = within(dialog).getByRole('combobox');
    await waitFor(() => expect(document.activeElement).toBe(input));
    // The first row is highlighted from the start, so Enter alone runs it.
    const options = within(dialog).getAllByRole('option');
    expect(options.map((o) => o.textContent)).toEqual([
      expect.stringContaining('New thread'),
      'Open project',
      'Settings',
    ]);
    await waitFor(() => expect(options[0]?.hasAttribute('data-highlighted')).toBe(true));
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    await waitFor(() =>
      expect(within(dialog).getByRole('option', { name: 'Open project' })).toHaveProperty(
        'dataset.highlighted',
        '',
      ),
    );
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(picked).toEqual(['open']);
  });

  it('filters as you type and says when nothing matches', async () => {
    render(<CommandPalette open onOpenChange={() => {}} items={ITEMS} onSelect={() => {}} />);
    const dialog = await screen.findByRole('dialog');
    const input = within(dialog).getByRole('combobox');
    fireEvent.change(input, { target: { value: 'sett' } });
    await waitFor(() =>
      expect(
        within(dialog)
          .getAllByRole('option')
          .map((o) => o.textContent),
      ).toEqual(['Settings']),
    );
    fireEvent.change(input, { target: { value: 'zzz' } });
    expect(await within(dialog).findByText('No results.')).toBeTruthy();
  });

  it('shows group headings and shortcuts', async () => {
    render(<CommandPalette open onOpenChange={() => {}} items={ITEMS} onSelect={() => {}} />);
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText('Threads')).toBeTruthy();
    expect(within(dialog).getByText('Projects')).toBeTruthy();
    expect(dialog.querySelectorAll('kbd kbd')).toHaveLength(2);
  });

  it('virtualises a long list and still reaches every row by keyboard', async () => {
    const many: CommandItem[] = Array.from({ length: 500 }, (_, i) => ({
      id: `f${i}`,
      text: `file-${i}.ts`,
    }));
    const picked: string[] = [];
    render(
      <CommandPalette
        open
        onOpenChange={() => {}}
        items={many}
        onSelect={(item) => picked.push(item.id)}
      />,
    );
    const dialog = await screen.findByRole('dialog');
    // Only a window of rows is in the DOM.
    const rendered = within(dialog).getAllByRole('option').length;
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(100);
    const input = within(dialog).getByRole('combobox');
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(picked).toEqual(['f1']);
  });

  it('closes on Escape', async () => {
    let open = true;
    render(
      <CommandPalette
        open
        onOpenChange={(o) => {
          open = o;
        }}
        items={ITEMS}
        onSelect={() => {}}
      />,
    );
    const dialog = await screen.findByRole('dialog');
    fireEvent.keyDown(within(dialog).getByRole('combobox'), { key: 'Escape' });
    expect(open).toBe(false);
  });
});

/* sidebar_resize_keyboard ------------------------------------------------------------ */

describe('Sidebar', () => {
  function Resizable({ side = 'left' as 'left' | 'right' }) {
    const [width, setWidth] = useState(280);
    return (
      <Sidebar
        label="Threads"
        side={side}
        width={width}
        onWidthChange={setWidth}
        min={200}
        max={480}
      >
        <SidebarContent>rows</SidebarContent>
      </Sidebar>
    );
  }

  it('sidebar_resize_keyboard', () => {
    render(<Resizable />);
    const nav = screen.getByRole('navigation', { name: 'Threads' });
    const handle = screen.getByRole('separator', { name: 'Resize threads' });
    expect(handle.getAttribute('aria-orientation')).toBe('vertical');
    expect(handle.getAttribute('aria-valuenow')).toBe('280');
    expect(handle.getAttribute('aria-valuemin')).toBe('200');
    expect(handle.getAttribute('aria-valuemax')).toBe('480');
    expect(handle.getAttribute('aria-controls')).toBe(nav.id);
    fireEvent.keyDown(handle, { key: 'ArrowRight' });
    expect(handle.getAttribute('aria-valuenow')).toBe('304');
    fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    expect(handle.getAttribute('aria-valuenow')).toBe('256');
    fireEvent.keyDown(handle, { key: 'End' });
    expect(handle.getAttribute('aria-valuenow')).toBe('480');
    fireEvent.keyDown(handle, { key: 'ArrowRight' });
    expect(handle.getAttribute('aria-valuenow')).toBe('480');
    fireEvent.keyDown(handle, { key: 'Home' });
    expect(handle.getAttribute('aria-valuenow')).toBe('200');
    expect(nav.parentElement?.style.width).toBe('200px');
  });

  it('a right-hand sidebar grows with the left arrow', () => {
    render(<Resizable side="right" />);
    const handle = screen.getByRole('separator', { name: 'Resize threads' });
    fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    expect(handle.getAttribute('aria-valuenow')).toBe('304');
  });

  it('a collapsed sidebar renders nothing and has no handle', () => {
    render(
      <Sidebar label="Threads" open={false} width={280} onWidthChange={() => {}}>
        rows
      </Sidebar>,
    );
    expect(screen.queryByRole('navigation')).toBeNull();
    expect(screen.queryByRole('separator')).toBeNull();
  });

  it('marks the active row for assistive technology', () => {
    render(
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton active>One</SidebarMenuButton>
        </SidebarMenuItem>
        <SidebarMenuItem>
          <SidebarMenuButton>Two</SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>,
    );
    expect(screen.getByRole('button', { name: 'One' }).getAttribute('aria-current')).toBe('page');
    expect(screen.getByRole('button', { name: 'Two' }).hasAttribute('aria-current')).toBe(false);
  });

  it('a horizontal handle resizes with the up and down keys', () => {
    // A drawer below its handle: the region is after the handle, so up grows it.
    function Drawer() {
      const [h, setH] = useState(240);
      return (
        <ResizeHandle
          label="Resize terminal"
          orientation="horizontal"
          edge="start"
          value={h}
          min={120}
          max={520}
          onResize={setH}
        />
      );
    }
    render(<Drawer />);
    const handle = screen.getByRole('separator', { name: 'Resize terminal' });
    expect(handle.getAttribute('aria-orientation')).toBe('horizontal');
    fireEvent.keyDown(handle, { key: 'ArrowUp' });
    expect(handle.getAttribute('aria-valuenow')).toBe('264');
    fireEvent.keyDown(handle, { key: 'ArrowDown' });
    fireEvent.keyDown(handle, { key: 'ArrowDown' });
    expect(handle.getAttribute('aria-valuenow')).toBe('216');
  });
});

/* menu_typeahead -------------------------------------------------------------------- */

describe('Menu', () => {
  it('menu_typeahead', async () => {
    render(
      <Menu trigger={<Button>Thread</Button>}>
        <MenuItem>Archive</MenuItem>
        <MenuItem>Rename</MenuItem>
        <MenuItem>Restore</MenuItem>
        <MenuItem>Delete</MenuItem>
      </Menu>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Thread' }));
    const menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getAllByRole('menuitem')).toHaveLength(4));
    // One letter moves to the first item that starts with it…
    fireEvent.keyDown(menu, { key: 'r' });
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Rename' })),
    );
    // …and typing on narrows by prefix.
    for (const key of ['e', 's']) fireEvent.keyDown(document.activeElement as Element, { key });
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Restore' })),
    );
  });
});

/* The rest: behaviour and ARIA ------------------------------------------------------ */

describe('ToggleGroup', () => {
  it('presses several options with multiple, and reports the set', () => {
    function T() {
      const [v, setV] = useState<string[]>(['wrap']);
      return (
        <ToggleGroup label="Options" multiple value={v} onValueChange={setV}>
          <Toggle value="wrap">Wrap</Toggle>
          <Toggle value="ws">Whitespace</Toggle>
        </ToggleGroup>
      );
    }
    render(<T />);
    expect(screen.getByRole('group', { name: 'Options' })).toBeTruthy();
    const ws = screen.getByRole('button', { name: 'Whitespace' });
    fireEvent.click(ws);
    expect(ws.getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByRole('button', { name: 'Wrap' }).getAttribute('aria-pressed')).toBe('true');
  });

  it('a lone Toggle is a pressed button', () => {
    function T() {
      const [on, setOn] = useState(false);
      return (
        <Toggle pressed={on} onPressedChange={setOn} aria-label="Pin">
          ★
        </Toggle>
      );
    }
    render(<T />);
    const pin = screen.getByRole('button', { name: 'Pin' });
    fireEvent.click(pin);
    expect(pin.getAttribute('aria-pressed')).toBe('true');
  });
});

describe('Switch', () => {
  it('is a labelled switch that toggles', () => {
    function T() {
      const [on, setOn] = useState(false);
      return <Switch label="Auto-approve" checked={on} onCheckedChange={setOn} />;
    }
    render(<T />);
    const sw = screen.getByRole('switch', { name: 'Auto-approve' });
    expect(sw.getAttribute('aria-checked')).toBe('false');
    fireEvent.click(sw);
    expect(sw.getAttribute('aria-checked')).toBe('true');
  });

  it('shows mixed without losing the real state', () => {
    render(<Switch label="All" mixed checked onCheckedChange={() => {}} />);
    expect(screen.getByRole('switch', { name: 'All' }).getAttribute('aria-checked')).toBe('mixed');
  });
});

describe('Collapsible', () => {
  it('expands its panel from the trigger', async () => {
    render(
      <Collapsible>
        <CollapsibleTrigger>Details</CollapsibleTrigger>
        <CollapsiblePanel>Body</CollapsiblePanel>
      </Collapsible>,
    );
    const trigger = screen.getByRole('button', { name: 'Details' });
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(trigger);
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
    expect(await screen.findByText('Body')).toBeTruthy();
  });
});

describe('AlertDialog', () => {
  it('is an alertdialog that confirms or cancels', async () => {
    let confirmed = 0;
    let closed = 0;
    render(
      <AlertDialog
        open
        onClose={() => closed++}
        title="Delete thread?"
        confirm="Delete"
        tone="danger"
        onConfirm={() => confirmed++}
      >
        This cannot be undone.
      </AlertDialog>,
    );
    const dialog = await screen.findByRole('alertdialog', { name: 'Delete thread?' });
    expect(dialog.getAttribute('aria-describedby')).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
    expect(confirmed).toBe(1);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(closed).toBe(1);
  });

  it('holds while pending', async () => {
    render(
      <AlertDialog
        open
        pending
        onClose={() => {}}
        title="Deleting"
        confirm="Delete"
        onConfirm={() => {}}
      />,
    );
    const dialog = await screen.findByRole('alertdialog');
    expect(
      (within(dialog).getByRole('button', { name: 'Delete' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe('Sheet', () => {
  it('is a modal dialog from one side that Escape closes', async () => {
    let closed = 0;
    render(
      <Sheet open onClose={() => closed++} title="Projects" side="left">
        <p>list</p>
      </Sheet>,
    );
    const sheet = await screen.findByRole('dialog', { name: 'Projects' });
    expect(sheet.getAttribute('data-side')).toBe('left');
    fireEvent.keyDown(sheet, { key: 'Escape' });
    expect(closed).toBe(1);
  });
});

describe('InputGroup', () => {
  it('is a group whose addon focuses the input', () => {
    render(
      <InputGroup>
        <InputGroupAddon>⌕</InputGroupAddon>
        <InputGroupInput aria-label="Search threads" />
        <InputGroupAddon align="end">
          <Kbd>/</Kbd>
        </InputGroupAddon>
      </InputGroup>,
    );
    expect(screen.getByRole('group')).toBeTruthy();
    fireEvent.mouseDown(screen.getByText('⌕'));
    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Search threads' }));
  });
});

describe('MiddleTruncate', () => {
  it('keeps the end that tells two values apart', () => {
    // A long last segment keeps a fixed ten characters.
    expect(splitForMiddleTruncate('fix/cache-main-20260918-180825')).toEqual({
      head: 'fix/cache-main-20260',
      tail: '918-180825',
    });
    expect(splitForMiddleTruncate('src/components/chat/Timeline.tsx')).toEqual({
      head: 'src/components/chat/',
      tail: 'Timeline.tsx',
    });
    expect(splitForMiddleTruncate('main')).toBeNull();
    // Code points, never half a surrogate pair.
    expect(splitForMiddleTruncate('😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀', 3)?.tail).toBe('😀😀😀');
  });

  it('reads as the whole string', () => {
    render(<MiddleTruncate value="src/components/chat/Timeline.tsx" />);
    const el = screen.getByTitle('src/components/chat/Timeline.tsx');
    expect(el.textContent).toBe('src/components/chat/Timeline.tsx');
  });
});

describe('Spinner, Skeleton, Kbd, ScrollArea', () => {
  it('a spinner is a labelled status', () => {
    render(<Spinner label="Starting the run" />);
    expect(screen.getByRole('status', { name: 'Starting the run' })).toBeTruthy();
  });

  it('a skeleton is hidden from assistive technology', () => {
    const { container } = render(<Skeleton className="h-4" />);
    expect(container.firstElementChild?.getAttribute('aria-hidden')).toBe('true');
  });

  it('a skeleton list is one labelled status over hidden bars (UX-4)', () => {
    const { container, rerender } = render(<SkeletonList label="Loading collections" rows={3} />);
    const status = screen.getByRole('status', { name: 'Loading collections' });
    expect(status.querySelectorAll('[data-slot="skeleton"][aria-hidden="true"]')).toHaveLength(3);
    rerender(<SkeletonList label="Loading the graph" layout="cards" />);
    expect(container.querySelectorAll('[data-slot="skeleton"].h-24')).toHaveLength(4);
  });

  it('kbd nests keys inside a group', () => {
    const { container } = render(
      <KbdGroup>
        <Kbd>Ctrl</Kbd>
        <Kbd>K</Kbd>
      </KbdGroup>,
    );
    expect(container.querySelectorAll('kbd > kbd')).toHaveLength(2);
  });

  it('a labelled scroll area is a reachable region', () => {
    render(
      <ScrollArea label="Log">
        <p>line</p>
      </ScrollArea>,
    );
    const region = screen.getByRole('region', { name: 'Log' });
    expect(region.getAttribute('tabindex')).toBe('0');
  });
});
