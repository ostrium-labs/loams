import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { parse } from 'yaml';
import { CatalogPage } from '../src/catalog-page.js';
import { DetailPage } from '../src/detail-page.js';
import { buildConfig, buildFields, exportYaml, leaves } from '../src/form.js';
import { fakeDesktop, load } from './fake.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const yamlText = () => (screen.getByLabelText('Instance YAML') as HTMLElement).textContent ?? '';

async function openDetail(id: string, ids = [id]) {
  const { api, clipboard } = fakeDesktop(ids);
  render(<DetailPage desktop={api} id={id} navigate={() => undefined} />);
  await screen.findByRole('form', { name: /configure/i });
  return { api, clipboard };
}

describe('connectors catalog page', () => {
  it('lists cards, filters by search and chip', async () => {
    const { api } = fakeDesktop(['kafka', 'adyen', 'postgresql']);
    const nav: string[] = [];
    render(<CatalogPage desktop={api} navigate={(p) => void nav.push(p)} />);
    expect(await screen.findAllByRole('article')).toHaveLength(3);
    fireEvent.change(screen.getByLabelText('Search connectors'), { target: { value: 'kaf' } });
    const cards = screen.getAllByRole('article');
    expect(cards).toHaveLength(1);
    fireEvent.click(within(cards[0] as HTMLElement).getByRole('button', { name: 'Details' }));
    expect(nav).toEqual(['/connectors/kafka']);
    fireEvent.change(screen.getByLabelText('Search connectors'), { target: { value: '' } });
    fireEvent.click(
      within(screen.getByRole('group', { name: 'Filter by status' })).getByRole('button', {
        name: 'planned',
      }),
    );
    expect(screen.getAllByRole('article')).toHaveLength(1);
    expect(screen.getByText('Adyen')).toBeTruthy();
  });
});

describe('connector detail and configure form', () => {
  it('form_renders_kafka_schema_required_fields', async () => {
    await openDetail('kafka');
    for (const key of ['brokers', 'topics', 'group_id_prefix']) {
      const input = screen.getByLabelText(new RegExp(`^${key}`)) as HTMLInputElement;
      expect(input.required).toBe(true);
    }
    const optional = screen.getByLabelText(/^group_id$/) as HTMLInputElement;
    expect(optional.required).toBe(false);
    // enum, boolean and nested objects render too.
    expect(screen.getByLabelText(/^acks/).tagName).toBe('SELECT');
    expect(screen.getByLabelText(/^enable_idempotence/).tagName).toBe('SELECT');
    expect(screen.getByText('sasl')).toBeTruthy();
    // Detail facts.
    expect(screen.getByText(/Connector runtime not yet available \(CN1\)/)).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Run' }).hasAttribute('disabled')).toBe(true);
    expect(screen.getByText('rdkafka 0.39 / librdkafka 2.x (verify)')).toBeTruthy();
  });

  it('secret_fields_password_type', async () => {
    await openDetail('kafka');
    const pw = screen.getByLabelText(/^password/) as HTMLInputElement;
    expect(pw.type).toBe('password');
    const key = screen.getByLabelText(/^key_pem/) as HTMLInputElement;
    expect(key.type).toBe('password');
    expect((screen.getByLabelText(/^username/) as HTMLInputElement).type).toBe('text');
  });

  it('secret values reach neither the YAML, the clipboard nor browser storage', async () => {
    const { clipboard } = await openDetail('kafka');
    const set = (re: RegExp, value: string) =>
      fireEvent.change(screen.getByLabelText(re), { target: { value } });
    set(/^brokers/, 'b1:9092');
    set(/^topics/, 'orders\nrefunds');
    set(/^group_id_prefix/, 'loams-dev');
    set(/^username/, 'svc');
    set(/^password/, 'hunter2-SECRET');
    set(/^key_pem/, '-----BEGIN PRIVATE KEY-----abc');
    await waitFor(() => expect(screen.getByText('Valid')).toBeTruthy());
    const text = yamlText();
    expect(text).not.toContain('hunter2');
    expect(text).not.toContain('BEGIN PRIVATE KEY');
    expect(text).toContain('${secret:sasl.password}');
    expect(text).toContain('${secret:tls.key_pem}');
    fireEvent.click(screen.getByRole('button', { name: 'Copy YAML' }));
    await waitFor(() => expect(clipboard).toHaveLength(1));
    expect(clipboard[0]).not.toContain('hunter2');
    for (const store of [localStorage, sessionStorage]) {
      for (let i = 0; i < store.length; i++) {
        expect(store.getItem(store.key(i) as string)).not.toContain('hunter2');
      }
    }
  });

  it('shows validation problems and blocks the export', async () => {
    const { clipboard } = await openDetail('kafka');
    await waitFor(() => expect(screen.getByText(/problem/)).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'Copy YAML' }));
    expect(await screen.findByText('Fix the highlighted fields first.')).toBeTruthy();
    expect(clipboard).toHaveLength(0);
    expect(
      screen.getAllByText(/must have required property|must NOT have fewer/).length,
    ).toBeGreaterThan(0);
  });

  it('a planned stub says there is nothing to configure', async () => {
    const { api } = fakeDesktop(['adyen']);
    render(<DetailPage desktop={api} id="adyen" navigate={() => undefined} />);
    expect(await screen.findByText('No configuration fields yet')).toBeTruthy();
    expect(screen.queryByRole('form')).toBeNull();
  });
});

describe('form model', () => {
  it('export_yaml_valid', () => {
    const { detail } = load('kafka');
    const fields = buildFields(detail.schema, detail.manifest.secrets as string[]);
    expect(leaves(fields).find((f) => f.id === 'sasl.password')?.secret).toBe(true);
    expect(leaves(fields).find((f) => f.id === 'brokers')?.required).toBe(true);
    const config = buildConfig(fields, {
      brokers: 'b1:9092',
      topics: 'orders, refunds',
      group_id_prefix: 'loams-dev',
      max_poll_records: '500',
      acks: 'all',
      enable_idempotence: 'true',
      'sasl.username': 'svc',
      'sasl.password': 'hunter2-SECRET',
    });
    const text = exportYaml({ connector: 'kafka', name: 'orders-in', config });
    expect(text).not.toContain('hunter2');
    const doc = parse(text);
    expect(doc.apiVersion).toBe('loams.flow/v1');
    expect(doc.kind).toBe('ConnectorInstance');
    expect(doc.metadata.name).toBe('orders-in');
    expect(doc.spec.connector).toBe('kafka');
    expect(doc.spec.config).toEqual({
      brokers: 'b1:9092',
      topics: ['orders', 'refunds'],
      group_id_prefix: 'loams-dev',
      max_poll_records: 500,
      acks: 'all',
      enable_idempotence: true,
      sasl: { username: 'svc', password: '${secret:sasl.password}' },
    });
  });

  it('omits untouched optional objects', () => {
    const { detail } = load('kafka');
    const fields = buildFields(detail.schema, detail.manifest.secrets as string[]);
    expect(buildConfig(fields, { brokers: 'b:1' })).toEqual({ brokers: 'b:1' });
  });
});
