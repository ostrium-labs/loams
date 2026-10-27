// Settings › Billing › Alerts (design 63 §18, §15): threshold rules, webhook endpoints and the
// delivery log. Threshold and lifecycle events are `io.loams.dev.meter.*` CloudEvents, sent
// signed (Standard Webhooks) to each endpoint; a new endpoint's secret is shown once.

import {
  Badge,
  Button,
  Card,
  Checkbox,
  ConfirmDialog,
  Field,
  Input,
  Notice,
  Snippet,
  Table,
} from '@loams/ui';
import { useState } from 'react';
import { formatTime } from './format.js';
import { errorText, idempotencyKey, Loaded, useLoad } from './load.js';
import type { AlertsReport, BillingClient, Delivery, ThresholdRule } from './types.js';

type Channel = ThresholdRule['channels'][number];
const CHANNELS: Channel[] = ['console', 'email', 'webhook'];

/** "80, 100" → [80, 100]; undefined when any part is not a percentage above zero. */
export function parseThresholds(text: string): number[] | undefined {
  const parts = text
    .split(/[,\s]+/)
    .map((p) => p.replace(/%$/, ''))
    .filter(Boolean);
  if (parts.length === 0) return undefined;
  const values = parts.map(Number);
  if (values.some((v) => !Number.isInteger(v) || v <= 0 || v > 1000)) return undefined;
  return [...new Set(values)].sort((a, b) => a - b);
}

function deliveryState(d: Delivery) {
  if (d.status >= 200 && d.status < 300) return <Badge variant="success">{d.status}</Badge>;
  return <Badge variant="danger">{d.status === 0 ? 'unreachable' : d.status}</Badge>;
}

function RuleForm({ client, onDone }: { client: BillingClient; onDone: () => void }) {
  const [feature, setFeature] = useState('');
  const [thresholds, setThresholds] = useState('80, 100');
  const [spendCap, setSpendCap] = useState(true);
  const [channels, setChannels] = useState<Channel[]>(['console', 'email']);
  const [error, setError] = useState<string>();
  const [key, setKey] = useState(idempotencyKey);
  const parsed = parseThresholds(thresholds);
  return (
    <form
      className="grid gap-3"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!parsed || channels.length === 0) return;
        try {
          await client.createRule(
            {
              feature: feature.trim() || undefined,
              thresholds: parsed,
              spendCap,
              channels,
            },
            key,
          );
          setKey(idempotencyKey());
          setError(undefined);
          onDone();
        } catch (err) {
          setError(errorText(err));
        }
      }}
    >
      {error && (
        <Notice tone="danger" title="Rule not added">
          {error}
        </Notice>
      )}
      <Field label="Feature" hint="Leave empty for every feature.">
        {(p) => <Input {...p} value={feature} onChange={(e) => setFeature(e.target.value)} />}
      </Field>
      <Field
        label="Thresholds (% of the period's allowance)"
        error={parsed ? undefined : 'Whole percentages above zero, separated by commas.'}
      >
        {(p) => <Input {...p} value={thresholds} onChange={(e) => setThresholds(e.target.value)} />}
      </Field>
      <Checkbox
        label="Also at these percentages of the spend cap"
        checked={spendCap}
        onChange={(e) => setSpendCap(e.target.checked)}
      />
      <fieldset className="flex gap-4">
        <legend className="text-small text-muted">Send to</legend>
        {CHANNELS.map((c) => (
          <Checkbox
            key={c}
            label={c}
            checked={channels.includes(c)}
            onChange={(e) =>
              setChannels((cs) => (e.target.checked ? [...cs, c] : cs.filter((x) => x !== c)))
            }
          />
        ))}
      </fieldset>
      <div>
        <Button type="submit" variant="primary" disabled={!parsed || channels.length === 0}>
          Add rule
        </Button>
      </div>
    </form>
  );
}

function WebhookForm({
  client,
  onCreated,
}: {
  client: BillingClient;
  onCreated: (secret: string) => void;
}) {
  const [url, setUrl] = useState('');
  const [error, setError] = useState<string>();
  const [key, setKey] = useState(idempotencyKey);
  return (
    <form
      className="flex flex-wrap items-end gap-2"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          const { secret } = await client.createWebhook(url.trim(), key);
          setUrl('');
          setKey(idempotencyKey());
          setError(undefined);
          onCreated(secret);
        } catch (err) {
          setError(errorText(err));
        }
      }}
    >
      <Field label="Endpoint URL" error={error}>
        {(p) => (
          <Input
            {...p}
            type="url"
            placeholder="https://"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
        )}
      </Field>
      <Button type="submit" disabled={!url.trim()}>
        Add endpoint
      </Button>
    </form>
  );
}

function AlertsBody({
  report,
  client,
  reload,
}: {
  report: AlertsReport;
  client: BillingClient;
  reload: () => void;
}) {
  const [secret, setSecret] = useState<string>();
  const [removing, setRemoving] = useState<ThresholdRule>();
  const [error, setError] = useState<string>();
  const hooks = new Map(report.webhooks.map((w) => [w.id, w.url]));
  return (
    <div className="grid gap-4">
      {!report.canEdit && (
        <Notice title="Read only">Changing alerts needs the billing admin role.</Notice>
      )}
      {error && (
        <Notice tone="danger" title="Not changed">
          {error}
        </Notice>
      )}
      {secret && (
        <Notice tone="success" title="Signing secret">
          <p className="mb-2">Copy it now; it is not shown again.</p>
          <Snippet prompt={false}>{secret}</Snippet>
        </Notice>
      )}
      <Card title="Threshold rules" flush>
        <Table
          caption="Threshold rules"
          columns={[
            {
              key: 'feature',
              header: 'Feature',
              cell: (r: ThresholdRule) => r.feature ?? 'Every feature',
            },
            {
              key: 'thresholds',
              header: 'At',
              cell: (r) =>
                `${r.thresholds.map((t) => `${t}%`).join(', ')}${r.spendCap ? ', and of the spend cap' : ''}`,
            },
            { key: 'channels', header: 'Sent to', cell: (r) => r.channels.join(', ') },
            {
              key: 'actions',
              header: '',
              cell: (r) =>
                report.canEdit ? (
                  <Button size="sm" variant="quiet" onClick={() => setRemoving(r)}>
                    Remove
                  </Button>
                ) : null,
            },
          ]}
          rows={report.rules}
          rowKey={(r) => r.id}
          empty={<p className="p-4 text-muted">No rules: nobody is told before usage stops.</p>}
        />
      </Card>
      {report.canEdit && (
        <Card title="Add a rule">
          <RuleForm client={client} onDone={reload} />
        </Card>
      )}
      <Card title="Webhook endpoints" flush>
        <Table
          caption="Webhook endpoints"
          columns={[
            { key: 'url', header: 'URL', cell: (w) => <code>{w.url}</code> },
            { key: 'types', header: 'Events', cell: (w) => w.types.join(', ') },
            { key: 'created', header: 'Added', cell: (w) => formatTime(w.createdAt) },
            {
              key: 'actions',
              header: '',
              cell: (w) =>
                report.canEdit ? (
                  <Button
                    size="sm"
                    variant="quiet"
                    onClick={async () => {
                      try {
                        setSecret((await client.rotateWebhookSecret(w.id)).secret);
                      } catch (err) {
                        setError(errorText(err));
                      }
                    }}
                  >
                    Rotate secret
                  </Button>
                ) : null,
            },
          ]}
          rows={report.webhooks}
          rowKey={(w) => w.id}
          empty={<p className="p-4 text-muted">No endpoints.</p>}
        />
        {report.canEdit && (
          <div className="p-4">
            <WebhookForm
              client={client}
              onCreated={(s) => {
                setSecret(s);
                reload();
              }}
            />
          </div>
        )}
      </Card>
      <Card title="Delivery log" flush>
        <Table
          caption="Webhook deliveries"
          columns={[
            { key: 'at', header: 'When', cell: (d: Delivery) => formatTime(d.at) },
            { key: 'type', header: 'Event', cell: (d) => <code>{d.type}</code> },
            { key: 'to', header: 'Endpoint', cell: (d) => hooks.get(d.webhook) ?? d.webhook },
            { key: 'status', header: 'Status', cell: deliveryState },
            { key: 'attempt', header: 'Attempt', numeric: true, cell: (d) => d.attempt },
          ]}
          rows={report.deliveries}
          rowKey={(d) => d.id}
          empty={<p className="p-4 text-muted">Nothing delivered yet.</p>}
        />
      </Card>
      <ConfirmDialog
        open={!!removing}
        onClose={() => setRemoving(undefined)}
        onConfirm={async () => {
          if (!removing) return;
          try {
            await client.deleteRule(removing.id);
            reload();
          } catch (err) {
            setError(errorText(err));
          }
          setRemoving(undefined);
        }}
        object="this threshold rule"
        confirm="Remove rule"
        impact="Nobody is told when usage crosses its thresholds."
      />
    </div>
  );
}

export function AlertsPage({ client }: { client: BillingClient }) {
  const [load, reload] = useLoad(() => client.alerts(), [client]);
  return (
    <Loaded load={load} what="alerts">
      {(report) => <AlertsBody report={report} client={client} reload={reload} />}
    </Loaded>
  );
}
