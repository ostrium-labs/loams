import type {
  LoamsDesktopApi,
  PgControlSettings,
  PgControlSettingsInput,
  PgControlTestResult,
} from '@loams/desktop/contracts';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { PgControlSection } from '../src/pg-control.js';

afterEach(cleanup);

type Api = NonNullable<LoamsDesktopApi['pgControl']>;

const base: PgControlSettings = {
  url: '',
  apiKeyFile: '',
  defaultUrl: 'http://127.0.0.1:7680',
  effectiveUrl: 'http://127.0.0.1:7680',
  envOverrides: [],
  keySource: 'none',
};

function fake(init: Partial<PgControlSettings> = {}, test?: PgControlTestResult) {
  let saved: PgControlSettings = { ...base, ...init };
  const saves: PgControlSettingsInput[] = [];
  const tests: PgControlSettingsInput[] = [];
  const listeners = new Set<(s: PgControlSettings) => void>();
  const api: Api = {
    settings: async () => saved,
    save: async (input) => {
      saves.push(input);
      if (input.apiKeyFile.includes('missing'))
        return {
          ok: false,
          code: 'invalid_argument',
          message: 'Some values need fixing.',
          errors: { apiKeyFile: 'No file at this path.' },
        };
      const url = input.url && !input.url.includes('://') ? `http://${input.url}` : input.url;
      saved = {
        ...saved,
        url,
        apiKeyFile: input.apiKeyFile,
        effectiveUrl: url || base.defaultUrl,
        keySource: input.apiKeyFile ? 'settings-file' : 'none',
      };
      return { ok: true, value: saved };
    },
    test: async (input) => {
      tests.push(input);
      return (
        test ?? {
          ok: true,
          code: 'ok',
          message: 'pg-control answered in 3 ms and accepted the API key.',
          url: 'http://127.0.0.1:7680',
          ms: 3,
        }
      );
    },
    pickKeyFile: async () => '/home/me/.config/loams/pg-api-key',
    onChanged: (cb) => {
      listeners.add(cb);
      return () => void listeners.delete(cb);
    },
  };
  const emit = (s: PgControlSettings) => {
    for (const l of listeners) l(s);
  };
  return { api, saves, tests, emit };
}

const urlInput = () => screen.getByLabelText('pg-control URL') as HTMLInputElement;
const keyInput = () => screen.getByLabelText('API key file') as HTMLInputElement;
const button = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement;

describe('PgControlSection', () => {
  it('shows the saved values and where requests go, never a key', async () => {
    const { api } = fake({
      url: 'http://10.0.0.5:7681',
      apiKeyFile: '/keys/pg',
      effectiveUrl: 'http://10.0.0.5:7681',
      keySource: 'settings-file',
    });
    render(<PgControlSection api={api} />);
    await waitFor(() => expect(urlInput().value).toBe('http://10.0.0.5:7681'));
    expect(keyInput().value).toBe('/keys/pg');
    expect(screen.getByTestId('pg-control-effective').textContent).toBe(
      'With the saved settings, requests go to http://10.0.0.5:7681 with the key from the saved key file.',
    );
    // Nothing to save until a value changes.
    expect(button('Save').disabled).toBe(true);
  });

  it('checks the URL as it is typed and will not save a bad one', async () => {
    const { api, saves } = fake();
    render(<PgControlSection api={api} />);
    await waitFor(() => expect(urlInput().disabled).toBe(false));
    fireEvent.change(urlInput(), { target: { value: 'http://x:1/api' } });
    expect(screen.getByText("Use the listener's address only, without a path.")).toBeTruthy();
    expect(urlInput().getAttribute('aria-invalid')).toBe('true');
    expect(button('Save').disabled).toBe(true);
    expect(button('Test connection').disabled).toBe(true);
    fireEvent.change(urlInput(), { target: { value: '127.0.0.1:7690' } });
    expect(urlInput().getAttribute('aria-invalid')).toBeNull();
    fireEvent.click(button('Save'));
    await screen.findByText('The Postgres page uses these settings now.');
    expect(saves).toEqual([{ url: '127.0.0.1:7690', apiKeyFile: '' }]);
    expect(urlInput().value).toBe('http://127.0.0.1:7690');
  });

  it('chooses the key file with the picker and shows the errors main finds on the field', async () => {
    const { api, saves } = fake();
    render(<PgControlSection api={api} />);
    await waitFor(() => expect(keyInput().disabled).toBe(false));
    fireEvent.click(button('Choose file'));
    await waitFor(() => expect(keyInput().value).toBe('/home/me/.config/loams/pg-api-key'));
    fireEvent.change(keyInput(), { target: { value: '/keys/missing' } });
    fireEvent.click(button('Save'));
    await screen.findByText('No file at this path.');
    expect(keyInput().getAttribute('aria-invalid')).toBe('true');
    expect(saves).toHaveLength(1);
    // Editing the field clears its error.
    fireEvent.change(keyInput(), { target: { value: '/keys/pg' } });
    expect(screen.queryByText('No file at this path.')).toBeNull();
  });

  it('tests the values in the form, saved or not', async () => {
    const { api, tests } = fake();
    render(<PgControlSection api={api} />);
    await waitFor(() => expect(urlInput().disabled).toBe(false));
    fireEvent.change(keyInput(), { target: { value: '/keys/pg' } });
    fireEvent.click(button('Test connection'));
    await screen.findByText('Connected');
    expect(screen.getByText(/accepted the API key/)).toBeTruthy();
    expect(tests).toEqual([{ url: '', apiKeyFile: '/keys/pg' }]);
  });

  it('shows a failed test', async () => {
    const { api } = fake(
      {},
      {
        ok: false,
        code: 'unauthenticated',
        message: 'pg-control refused the API key. Mint a new one with pg-api-key.',
        url: 'http://127.0.0.1:7680',
      },
    );
    render(<PgControlSection api={api} />);
    await waitFor(() => expect(urlInput().disabled).toBe(false));
    fireEvent.click(button('Test connection'));
    await screen.findByText('Could not connect');
    expect(screen.getByText(/refused the API key/)).toBeTruthy();
  });

  it('says which environment variables win, and follows changes saved elsewhere', async () => {
    const { api, emit } = fake({
      envOverrides: ['LOAMS_PG_CONTROL_URL'],
      effectiveUrl: 'http://127.0.0.1:9999',
    });
    render(<PgControlSection api={api} />);
    await screen.findByText('Set in the environment');
    expect(screen.getByText(/LOAMS_PG_CONTROL_URL is set for this app and wins/)).toBeTruthy();
    emit({ ...base, url: 'http://10.1.1.1:7680', effectiveUrl: 'http://10.1.1.1:7680' });
    await waitFor(() => expect(urlInput().value).toBe('http://10.1.1.1:7680'));
    expect(screen.queryByText('Set in the environment')).toBeNull();
  });
});
