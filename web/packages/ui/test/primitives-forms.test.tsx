import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  ButtonGroup,
  buttonClass,
  CheckboxControl,
  CollapsibleSectionHeader,
  Empty,
  InputControl,
  NumberField,
  NumberFieldGroup,
  NumberFieldIncrement,
  NumberFieldInput,
  Radio,
  RadioGroup,
  SelectItem,
  SelectPopup,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  Separator,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRoot,
  TableRow,
} from '../src';

afterEach(cleanup);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.getAnimations ??= () => [];

describe('Button (vendored, Loams variants kept)', () => {
  it('is a type=button with the Loams class names and a data-slot', () => {
    render(<Button variant="primary">Save</Button>);
    const b = screen.getByRole('button', { name: 'Save' });
    expect(b.getAttribute('type')).toBe('button');
    expect(b.dataset.slot).toBe('button');
    expect(b.className).toContain('loams-btn loams-btn-primary');
  });

  it('renders as another element with render, and keeps buttonClass for old callers', () => {
    render(<Button render={<a href="#/x" />}>Open</Button>);
    const a = screen.getByRole('link', { name: 'Open' });
    expect(a.getAttribute('type')).toBeNull();
    expect(buttonClass({ size: 'sm' })).toBe('loams-btn loams-btn-secondary loams-btn-sm');
    expect(buttonClass()).toBe('loams-btn loams-btn-secondary');
  });
});

describe('Badge', () => {
  it('keeps the old call form and adds state variants', () => {
    render(
      <>
        <Badge title="edition">OSS</Badge>
        <Badge variant="danger">Failed</Badge>
      </>,
    );
    const oss = screen.getByText('OSS');
    expect(oss.dataset.slot).toBe('badge');
    expect(oss.getAttribute('title')).toBe('edition');
    expect(screen.getByText('Failed').className).toContain('bg-danger-soft');
  });
});

describe('form controls', () => {
  it('InputControl wraps an input and shows invalid state', () => {
    render(<InputControl aria-label="Name" aria-invalid />);
    const input = screen.getByRole('textbox', { name: 'Name' });
    expect(input.closest('[data-slot="input-control"]')?.className).toContain(
      'has-aria-invalid:border-danger',
    );
  });

  it('CheckboxControl toggles with a click', () => {
    render(<CheckboxControl aria-label="Remember" />);
    const box = screen.getByRole('checkbox', { name: 'Remember' });
    expect(box.getAttribute('aria-checked')).toBe('false');
    fireEvent.click(box);
    expect(box.getAttribute('aria-checked')).toBe('true');
  });

  it('RadioGroup moves with the arrow keys', async () => {
    render(
      <RadioGroup aria-label="Size" defaultValue="s">
        <Radio value="s" aria-label="Small" />
        <Radio value="l" aria-label="Large" />
      </RadioGroup>,
    );
    const small = screen.getByRole('radio', { name: 'Small' });
    small.focus();
    fireEvent.keyDown(small, { key: 'ArrowDown' });
    // Base UI checks the next radio after the focus move settles.
    await act(async () => {});
    expect(screen.getByRole('radio', { name: 'Large' }).getAttribute('aria-checked')).toBe('true');
  });

  it('NumberField increments', () => {
    render(
      <NumberField defaultValue={2}>
        <NumberFieldGroup>
          <NumberFieldInput aria-label="Replicas" />
          <NumberFieldIncrement aria-label="More" />
        </NumberFieldGroup>
      </NumberField>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'More' }));
    expect((screen.getByRole('textbox', { name: 'Replicas' }) as HTMLInputElement).value).toBe('3');
  });

  it('SelectRoot opens and picks an item', async () => {
    const picked: unknown[] = [];
    render(
      <SelectRoot defaultValue="a" onValueChange={(v) => picked.push(v)}>
        <SelectTrigger aria-label="Region">
          <SelectValue />
        </SelectTrigger>
        <SelectPopup>
          <SelectItem value="a">Amsterdam</SelectItem>
          <SelectItem value="b">Berlin</SelectItem>
        </SelectPopup>
      </SelectRoot>,
    );
    fireEvent.click(screen.getByRole('combobox', { name: 'Region' }));
    const berlin = await screen.findByRole('option', { name: 'Berlin' });
    await act(async () => {});
    // Base UI picks on the pointer sequence, not on a bare click event (as studio-ui's git test).
    fireEvent.pointerDown(berlin, { pointerType: 'mouse' });
    fireEvent.pointerUp(berlin, { pointerType: 'mouse' });
    fireEvent.mouseUp(berlin);
    fireEvent.click(berlin);
    await act(async () => {});
    expect(picked).toEqual(['b']);
  });
});

describe('display', () => {
  it('ButtonGroup, Separator and Alert carry their roles', () => {
    render(
      <>
        <ButtonGroup aria-label="Zoom">
          <Button>−</Button>
          <Button>+</Button>
        </ButtonGroup>
        <Separator />
        <Alert variant="warning">
          <AlertTitle>Branch is protected</AlertTitle>
          <AlertDescription>Writes need approval.</AlertDescription>
        </Alert>
      </>,
    );
    expect(screen.getByRole('group', { name: 'Zoom' })).toBeTruthy();
    expect(screen.getByRole('separator')).toBeTruthy();
    expect(screen.getByRole('alert').className).toContain('bg-warn-soft');
  });

  it('Empty keeps its props and no longer paints grain (Q929)', () => {
    const { container } = render(
      <Empty title="No branches" seed={3} actions={<Button>Create</Button>}>
        Branch from main to try a change.
      </Empty>,
    );
    expect(screen.getByText('No branches')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Create' })).toBeTruthy();
    expect(container.querySelector('canvas')).toBeNull();
  });

  it('TableRoot is a semantic table and CollapsibleSectionHeader reports its state', () => {
    render(
      <>
        <TableRoot aria-label="Tables">
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow>
              <TableCell>orders</TableCell>
            </TableRow>
          </TableBody>
        </TableRoot>
        <CollapsibleSectionHeader expanded={false}>Archived</CollapsibleSectionHeader>
      </>,
    );
    expect(screen.getByRole('table', { name: 'Tables' })).toBeTruthy();
    expect(screen.getByRole('button', { name: /Archived/ }).getAttribute('aria-expanded')).toBe(
      'false',
    );
  });
});

describe('review follow-ups (#714)', () => {
  it('a Button rendered as a <button> keeps type=button', () => {
    // oxlint-disable-next-line react/button-has-type -- the point is that Button supplies the type
    render(<Button render={<button />}>Go</Button>);
    expect(screen.getByRole('button', { name: 'Go' }).getAttribute('type')).toBe('button');
  });

  it('Alert sorts the children of a fragment', () => {
    render(
      <Alert>
        <>
          <AlertTitle>Title</AlertTitle>
          <AlertDescription>Body</AlertDescription>
        </>
      </Alert>,
    );
    expect(screen.getByText('Title').parentElement?.className).toContain('flex-col');
  });
});
