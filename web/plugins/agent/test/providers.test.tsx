import type { ChatProviderInfo, IpcResult, LoamsDesktopApi } from '@loams/desktop/contracts';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { ProvidersSection } from '../src/providers.js';
import { AgentStore } from '../src/store.js';

afterEach(cleanup);

const mk = (over: Partial<ChatProviderInfo>): ChatProviderInfo => ({
  id: 'deepseek',
  label: 'DeepSeek',
  kind: 'openai',
  baseUrl: 'https://api.deepseek.com/v1',
  model: 'deepseek-chat',
  defaultModel: 'deepseek-chat',
  needsKey: true,
  hasKey: false,
  configured: false,
  persistent: true,
  ...over,
});
const ok = <T,>(value: T): IpcResult<T> => ({ ok: true, value });

function setup(providers: ChatProviderInfo[]) {
  const chat = {
    providers: vi.fn(async () => providers),
    configureProvider: vi.fn(
      async (_id: string, _cfg: unknown): Promise<IpcResult<ChatProviderInfo>> => ok(mk({})),
    ),
    testProvider: vi.fn(async (): Promise<IpcResult<{ model: string; ms: number }>> =>
      ok({ model: 'm', ms: 12 }),
    ),
    list: vi.fn(async () => []),
    onEvent: () => () => {},
  };
  const desktop = { chat } as unknown as LoamsDesktopApi;
  render(<ProvidersSection desktop={desktop} store={new AgentStore({ desktop })} />);
  return chat;
}
const card = async (label: string) =>
  within(await screen.findByRole('form', { name: `${label} settings` }));

describe('agent providers section', () => {
  it('never prefills the key and says a saved one is kept', async () => {
    setup([mk({ hasKey: true, configured: true })]);
    const c = await card('DeepSeek');
    const key = c.getByLabelText('API key') as HTMLInputElement;
    expect(key.type).toBe('password');
    expect(key.value).toBe('');
    expect(key.placeholder).toBe('Saved — leave blank to keep');
    // The hint does not repeat the placeholder.
    expect(c.queryAllByText(/leave blank to keep/)).toHaveLength(0);
  });

  it('saves with the key only when typed, and clears the field', async () => {
    const chat = setup([mk({ hasKey: true, configured: true })]);
    const c = await card('DeepSeek');
    fireEvent.change(c.getByLabelText('Model'), { target: { value: 'deepseek-reasoner' } });
    fireEvent.click(c.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(chat.configureProvider).toHaveBeenCalledTimes(1));
    expect(chat.configureProvider).toHaveBeenLastCalledWith('deepseek', {
      baseUrl: 'https://api.deepseek.com/v1',
      model: 'deepseek-reasoner',
    });
    // The first save holds the form busy ("Saving…", disabled) until it settles;
    // a click before then is ignored, so wait for Save to come back.
    await waitFor(() => expect(c.getByRole('button', { name: 'Save' })).toBeTruthy());
    fireEvent.change(c.getByLabelText('API key'), { target: { value: 'sk-new-key-123456' } });
    await waitFor(() =>
      expect((c.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(c.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(chat.configureProvider).toHaveBeenCalledTimes(2));
    expect(chat.configureProvider.mock.calls[1]?.[1]).toMatchObject({
      apiKey: 'sk-new-key-123456',
    });
    await waitFor(() => expect((c.getByLabelText('API key') as HTMLInputElement).value).toBe(''));
  });

  it('key_required_message_when_origin_changed', async () => {
    const chat = setup([mk({ hasKey: true, configured: true })]);
    chat.configureProvider.mockResolvedValueOnce({
      ok: false,
      code: 'key_required',
      message: 'The base URL now points at https://other.test. Enter the key for this one.',
    });
    const c = await card('DeepSeek');
    fireEvent.change(c.getByLabelText('Base URL'), { target: { value: 'https://other.test/v1' } });
    // A warning appears before saving, too.
    expect(await c.findByText('This is a different server')).toBeTruthy();
    fireEvent.click(c.getByRole('button', { name: 'Save' }));
    expect(await c.findByText('Enter the key for this server')).toBeTruthy();
    expect(c.getByText(/now points at https:\/\/other.test/)).toBeTruthy();
  });

  it('anthropic_fallback_toggle_is_off_by_default_and_explained', async () => {
    const chat = setup([
      mk({ id: 'anthropic', label: 'Anthropic', kind: 'anthropic', fallback: false }),
    ]);
    const c = await card('Anthropic');
    const box = c.getByLabelText(
      'Allow server-side fallback to another model on refusal',
    ) as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(c.getByText(/price applies/)).toBeTruthy();
    // The form resets itself to the saved values in an effect after its first
    // render; on a busy runner that can land after a click and undo it, and Save
    // stays disabled until the edit has rendered. So click until the edit holds
    // (only ever when it is not yet checked), and Save has enabled with it.
    const save = c.getByRole('button', { name: 'Save' }) as HTMLButtonElement;
    await waitFor(() => {
      if (!box.checked) fireEvent.click(box);
      expect(box.checked).toBe(true);
      expect(save.disabled).toBe(false);
    });
    fireEvent.click(save);
    await waitFor(() => expect(chat.configureProvider).toHaveBeenCalled());
    expect(chat.configureProvider.mock.calls[0]?.[1]).toMatchObject({ fallback: true });
  });

  it('other providers have no fallback toggle', async () => {
    setup([mk({})]);
    const c = await card('DeepSeek');
    expect(c.queryByText(/server-side fallback/)).toBeNull();
  });

  it('test_button_uses_saved_settings', async () => {
    const chat = setup([mk({ hasKey: true, configured: true })]);
    const c = await card('DeepSeek');
    const test = c.getByRole('button', { name: 'Test' }) as HTMLButtonElement;
    // The button's disabled state follows the edit on a later render; wait for it
    // rather than reading it at once (it raced on a busy CI runner).
    fireEvent.change(c.getByLabelText('Model'), { target: { value: 'x' } });
    await waitFor(() => expect(test.disabled).toBe(true));
    fireEvent.change(c.getByLabelText('Model'), { target: { value: 'deepseek-chat' } });
    await waitFor(() => expect(test.disabled).toBe(false));
    fireEvent.click(test);
    expect(await c.findByText('The provider answered')).toBeTruthy();
    expect(chat.testProvider).toHaveBeenCalledWith('deepseek');
    chat.testProvider.mockResolvedValueOnce({
      ok: false,
      code: 'test_failed',
      message: 'HTTP 401: bad key',
    });
    fireEvent.click(test);
    expect(await c.findByText('HTTP 401: bad key')).toBeTruthy();
  });

  it('keyless provider hides the key field; no keyring is called out', async () => {
    setup([
      mk({ id: 'ollama', label: 'Ollama', needsKey: false, configured: true, persistent: false }),
    ]);
    const c = await card('Ollama');
    expect(c.queryByLabelText('API key')).toBeNull();
    expect(screen.getByText('No system keyring')).toBeTruthy();
  });
});
