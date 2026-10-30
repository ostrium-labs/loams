# Loams Meter exporters

**Status: preview.** The exporters send what the meter has closed to a billing provider. Each one is optional and off by default, and data only flows one way: Loams never reads balances back. Design: [§63 §16](../design/63-loams-meter.md), decisions D2207 and D2403.

Each exporter needs two things:

- the `loams` build feature for it (`meter-export-stripe`, `meter-export-lago`, `meter-export-openmeter` or `meter-export-webhook`);
- its settings in the environment of every gateway node.

Secrets are read from files and are never logged.

## What runs, and when

Every 5 minutes, each node runs every configured exporter:

| Exporter | Settings | Sends |
|---|---|---|
| Stripe usage | `LOAMS_METER_STRIPE_API_KEY_FILE`; turn it off with `LOAMS_METER_STRIPE_EXPORT_USAGE=false` | For each customer with a Stripe customer id: each closed hour's **overage** of every metered `SUM` or `COUNT` feature, as Stripe meter events. It covers the current period, and the previous period for 26 hours after it ends. `included` is the grants' total at the period's start |
| Stripe payment state | the same key, plus `LOAMS_METER_STRIPE_WEBHOOK_SECRET_FILE` | Payment-method webhooks at `POST /v1/meter/webhooks/stripe`, and a daily poll |
| Lago | `LOAMS_METER_LAGO_URL`, `LOAMS_METER_LAGO_API_KEY_FILE` | Closed hour windows from the last 26 hours, for subjects that belong to a customer. A re-closed window sends only the difference |
| OpenMeter | `LOAMS_METER_OPENMETER_URL`; optional `LOAMS_METER_OPENMETER_TOKEN_FILE` and `LOAMS_METER_OPENMETER_TYPES` (comma-separated type prefixes; the default is every `io.loams.dev.usage.` type) | Raw usage events from `_system/_meter`, without `loamslease`. Each partition is read from the exporter's cursor. The first run starts at the beginning of the retained stream (14 days) |
| CloudEvents webhook | `LOAMS_METER_EXPORT_WEBHOOK_URL`, `LOAMS_METER_EXPORT_WEBHOOK_SECRET_FILE` | Closed hour windows as signed CloudEvents |

How the exporters behave:

- **Idempotency.** Every send goes to the export ledger before it is attempted. A retry after a crash or a timeout therefore resends the same Stripe `identifier`, Lago `transaction_id` or OpenMeter event id.
- **One sender per subject.** Stripe usage and OpenMeter run only on the stream partitions a node's counters own, so each subject is exported by exactly one node.
- **Stripe key permissions.** The key needs write access to meter events and read access to customers. If Stripe refuses the key (401 or 403), the usage export logs it once, shows it as a console notification for the affected customer, and retries hourly instead of every 5 minutes. Set `LOAMS_METER_STRIPE_EXPORT_USAGE=false` to turn the usage export off.
- **Stripe billing periods** must start on whole UTC hours. If a period does not, that customer's usage export stops and an error is logged.

## The mapping file

Set `LOAMS_METER_EXPORT_MAPPING_FILE` to a YAML or JSON file. Everything in it is optional, and anything it leaves out uses the default. Unknown keys are refused, so a typo stops the node from starting instead of being silently ignored.

```yaml
stripe:
  # Features to export; default: every metered SUM or COUNT feature.
  features: [house_cpu, gateway_requests]
  # Feature key -> the Stripe meter's event_name; default: the feature's meter slug.
  event_names:
    house_cpu: loams_house_cpu_ms
lago:
  # Meters to export; default: every meter.
  meters: [house_cpu_ms]
  # Meter slug -> the billable metric's code; default: the slug.
  codes:
    house_cpu_ms: house_cpu
  # Loams customer id (or a subject) -> external_subscription_id;
  # default: the Loams customer id.
  subscriptions:
    01JC8Z3V5M6Q2T9R4W7X1Y0ABC: sub_acme_2026
    org_7: sub_org7
```

On the provider side:

- **Stripe:** create one meter per exported feature. Give it the event name from the mapping (by default, the meter slug), `customer_mapping` on `stripe_customer_id`, the value in `value`, and `event_time_window: hour`.
- **Lago:** create a `sum_agg` billable metric on the property `value` with the mapped code. Give each customer a subscription whose `external_id` matches the mapping (by default, the Loams customer id).
