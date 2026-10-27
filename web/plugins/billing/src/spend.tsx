// Settings › Billing › Spend controls (design 63 §18, §13): overage opt-in, the spend cap and
// auto top-up. Overage needs a card and a cap; without a card the switch is off with the reason.
// Changes need `billing_admin` and a confirmation, and the meter audits them.

import { Button, Card, ConfirmDialog, Field, Input, Notice, Switch } from '@loams/ui';
import { useState } from 'react';
import { BILLING_ROUTES } from './banner.js';
import { formatMoney, moneyInputValue, parseMoneyInput } from './format.js';
import { errorText, idempotencyKey, Loaded, useLoad } from './load.js';
import type { BillingClient, SpendControls, SpendControlsUpdate } from './types.js';

/** Why overage cannot be turned on, or undefined when it can. */
export function overageBlocked(c: SpendControls): string | undefined {
  if (!c.canEdit) return 'Changing spend controls needs the billing admin role.';
  if (c.paymentState === 'none') {
    return 'Overage needs a payment method. Add a card on the Payment page first.';
  }
  return undefined;
}

function SpendForm({
  controls,
  client,
  navigate,
  onSaved,
}: {
  controls: SpendControls;
  client: BillingClient;
  navigate: (to: string) => void;
  onSaved: (c: SpendControls) => void;
}) {
  const currency = controls.spendCap?.currency ?? controls.suggestedCap.currency;
  const [overage, setOverage] = useState(controls.overageEnabled);
  const [cap, setCap] = useState(moneyInputValue(controls.spendCap ?? controls.suggestedCap));
  const [topUp, setTopUp] = useState(controls.autoTopUp?.enabled ?? false);
  const [below, setBelow] = useState(moneyInputValue(controls.autoTopUp?.below));
  const [amount, setAmount] = useState(moneyInputValue(controls.autoTopUp?.amount));
  const [confirming, setConfirming] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  // One key per confirmed change, so a retry of the same save is not applied twice.
  const [key, setKey] = useState(idempotencyKey);

  const blocked = overageBlocked(controls);
  const capMoney = parseMoneyInput(cap, currency);
  const capError = overage && !capMoney ? 'Enter the most overage may cost per period.' : undefined;
  const belowMoney = parseMoneyInput(below, currency);
  const amountMoney = parseMoneyInput(amount, currency);
  const topUpError =
    topUp && (!belowMoney || !amountMoney) ? 'Enter both amounts for auto top-up.' : undefined;
  const editable = controls.canEdit;

  const update: SpendControlsUpdate = {
    overageEnabled: overage,
    spendCap: overage ? capMoney : controls.spendCap,
    autoTopUp:
      belowMoney && amountMoney
        ? { enabled: topUp, below: belowMoney, amount: amountMoney }
        : controls.autoTopUp,
  };

  const save = async () => {
    setPending(true);
    setError(undefined);
    try {
      const saved = await client.setSpendControls(update, key);
      setConfirming(false);
      setKey(idempotencyKey());
      onSaved(saved);
    } catch (e) {
      setConfirming(false);
      setError(errorText(e));
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="grid gap-4">
      {!editable && (
        <Notice title="Read only">Changing spend controls needs the billing admin role.</Notice>
      )}
      {error && (
        <Notice tone="danger" title="Not saved">
          {error}
        </Notice>
      )}
      <Card title="Overage">
        <div className="grid gap-3">
          <Switch
            label="Allow overage past the included allowance"
            checked={overage}
            disabled={!!blocked && !controls.overageEnabled}
            onCheckedChange={setOverage}
          />
          {blocked && !controls.overageEnabled && (
            <p className="text-small text-muted" data-testid="overage-blocked">
              {blocked}{' '}
              {controls.paymentState === 'none' && (
                <Button variant="link" size="sm" onClick={() => navigate(BILLING_ROUTES.payment)}>
                  Payment
                </Button>
              )}
            </p>
          )}
          <Field
            label={`Spend cap per period (${currency})`}
            hint={`Overage stops here. Suggested: ${formatMoney(controls.suggestedCap)}, twice the plan's included value.`}
            error={capError}
          >
            {(p) => (
              <Input
                {...p}
                inputMode="decimal"
                value={cap}
                disabled={!editable || !overage}
                onChange={(e) => setCap(e.target.value)}
              />
            )}
          </Field>
        </div>
      </Card>
      <Card title="Auto top-up">
        <div className="grid gap-3">
          <Switch
            label="Buy prepaid credit automatically"
            checked={topUp}
            disabled={!editable || controls.paymentState === 'none'}
            onCheckedChange={setTopUp}
          />
          <Field label={`When credit falls below (${currency})`} error={topUpError}>
            {(p) => (
              <Input
                {...p}
                inputMode="decimal"
                value={below}
                disabled={!editable || !topUp}
                onChange={(e) => setBelow(e.target.value)}
              />
            )}
          </Field>
          <Field label={`Buy (${currency})`}>
            {(p) => (
              <Input
                {...p}
                inputMode="decimal"
                value={amount}
                disabled={!editable || !topUp}
                onChange={(e) => setAmount(e.target.value)}
              />
            )}
          </Field>
        </div>
      </Card>
      <div>
        <Button
          variant="primary"
          disabled={!editable || !!capError || !!topUpError}
          onClick={() => setConfirming(true)}
        >
          Save spend controls
        </Button>
      </div>
      <ConfirmDialog
        open={confirming}
        pending={pending}
        onClose={() => setConfirming(false)}
        onConfirm={save}
        object="this organisation's spend controls"
        confirm="Save spend controls"
        impact={
          overage && capMoney
            ? `Usage past the allowance will be billed per unit, up to ${formatMoney(capMoney)} each period. The change is audited.`
            : 'Usage stops at the allowance. The change is audited.'
        }
      />
    </div>
  );
}

export function SpendPage({
  client,
  navigate,
}: {
  client: BillingClient;
  navigate: (to: string) => void;
}) {
  const [load, reload] = useLoad(() => client.spendControls(), [client]);
  const [saved, setSaved] = useState(false);
  return (
    <Loaded load={load} what="spend controls">
      {(controls) => (
        <>
          {saved && (
            <div className="mb-3">
              <Notice tone="success" title="Spend controls saved" />
            </div>
          )}
          <SpendForm
            key={JSON.stringify(controls)}
            controls={controls}
            client={client}
            navigate={navigate}
            onSaved={() => {
              setSaved(true);
              reload();
            }}
          />
        </>
      )}
    </Loaded>
  );
}
