// "Run in parallel…" (SD1 Task 19; §53 §5.8): the composer's prompt sent to several
// harness and model variants at once, each in its own worktree from one base, as a
// daemon run group (`RunGroupService.CreateRunGroup`, Task 12). The composer offers it
// only when the daemon lists the `run-groups` feature.

import type { MessageInitShape } from '@bufbuild/protobuf';
import {
  ApprovalMode,
  type CreateRunGroupRequestSchema,
  type HarnessDescriptor,
  HarnessId,
} from '@loams/proto/agentd';
import { Button, Dialog, Field, Input, Select } from '@loams/ui';
import { Plus, X } from 'lucide-react';
import { useState } from 'react';

export const MIN_VARIANTS = 2;
export const MAX_VARIANTS = 4;

export interface RunVariantChoice {
  harness: HarnessId;
  model?: string;
}

export interface RunGroupChoices {
  variants: RunVariantChoice[];
  base: string;
  /** A project action that checks each member (for example the test suite). */
  checkActionId?: string;
  approvalMode: 'supervised' | 'full-access';
}

/** The `CreateRunGroup` request for a prompt and the dialog's choices. */
export function buildCreateRunGroupRequest(input: {
  spaceId?: string;
  repoPath: string;
  prompt: string;
  attachments: readonly string[];
  choices: RunGroupChoices;
}): MessageInitShape<typeof CreateRunGroupRequestSchema> {
  const { choices } = input;
  return {
    // `optional` on the wire: an empty id would name a project space that does not exist.
    ...(input.spaceId ? { spaceId: input.spaceId } : {}),
    repoPath: input.repoPath,
    base: choices.base,
    prompt: input.prompt,
    attachments: [...input.attachments],
    variants: choices.variants.map((v) => ({
      harness: v.harness,
      ...(v.model ? { model: v.model } : {}),
    })),
    ...(choices.checkActionId ? { checkActionId: choices.checkActionId } : {}),
    approvalMode:
      choices.approvalMode === 'full-access' ? ApprovalMode.FULL_ACCESS : ApprovalMode.SUPERVISED,
  };
}

function harnessName(harnesses: readonly HarnessDescriptor[], id: HarnessId): string {
  return harnesses.find((h) => h.id === id)?.name ?? HarnessId[id] ?? 'Harness';
}

export function RunInParallelDialog({
  open,
  onClose,
  harnesses,
  initial,
  branches,
  actions = [],
  pending,
  error,
  onSubmit,
}: {
  open: boolean;
  onClose: () => void;
  /** Installed and enabled harnesses. */
  harnesses: readonly HarnessDescriptor[];
  initial: { harness: HarnessId; model?: string; base: string };
  /** The project's branches for the base; a text field when unknown. */
  branches?: readonly string[];
  /** The project's actions, for the check. */
  actions?: readonly { id: string; name: string }[];
  pending?: boolean;
  error?: string;
  onSubmit: (choices: RunGroupChoices) => void;
}) {
  const second = harnesses.find((h) => h.id !== initial.harness)?.id ?? initial.harness;
  const [variants, setVariants] = useState<RunVariantChoice[]>(() => [
    { harness: initial.harness, ...(initial.model ? { model: initial.model } : {}) },
    { harness: second },
  ]);
  const [base, setBase] = useState(initial.base);
  const [check, setCheck] = useState('');
  const [approval, setApproval] = useState<RunGroupChoices['approvalMode']>('supervised');
  const valid =
    variants.length >= MIN_VARIANTS && variants.length <= MAX_VARIANTS && base.trim() !== '';
  const setVariant = (index: number, patch: Partial<RunVariantChoice>) =>
    setVariants((list) => list.map((v, i) => (i === index ? { ...v, ...patch } : v)));

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Run in parallel"
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={!valid || pending}
            onClick={() =>
              onSubmit({
                variants,
                base: base.trim(),
                ...(check ? { checkActionId: check } : {}),
                approvalMode: approval,
              })
            }
          >
            {pending ? 'Starting…' : `Start ${variants.length} runs`}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="m-0 text-[0.86rem] text-muted">
          Each variant runs this prompt in its own worktree from the same base. Compare them and
          keep one when they finish.
        </p>
        <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
          <legend className="mb-1 text-micro font-medium text-faint">Variants</legend>
          {variants.map((variant, index) => (
            // oxlint-disable-next-line react/no-array-index-key -- rows are positional.
            <div key={index} className="flex items-center gap-2">
              <Select
                aria-label={`Variant ${index + 1} harness`}
                value={String(variant.harness)}
                onChange={(e) =>
                  setVariant(index, {
                    harness: Number(e.target.value) as HarnessId,
                    model: undefined,
                  })
                }
                className="min-w-0 flex-1"
              >
                {harnesses.map((h) => (
                  <option key={h.id} value={String(h.id)}>
                    {h.name}
                  </option>
                ))}
              </Select>
              <Input
                aria-label={`Variant ${index + 1} model`}
                placeholder={`${harnessName(harnesses, variant.harness)} default model`}
                value={variant.model ?? ''}
                onChange={(e) => setVariant(index, { model: e.target.value.trim() || undefined })}
                className="min-w-0 flex-1"
              />
              <Button
                size="icon"
                variant="quiet"
                aria-label={`Remove variant ${index + 1}`}
                disabled={variants.length <= MIN_VARIANTS}
                onClick={() => setVariants((list) => list.filter((_, i) => i !== index))}
              >
                <X size={14} aria-hidden="true" />
              </Button>
            </div>
          ))}
          <div>
            <Button
              size="sm"
              variant="secondary"
              disabled={variants.length >= MAX_VARIANTS}
              onClick={() => setVariants((list) => [...list, { harness: initial.harness }])}
            >
              <Plus size={14} aria-hidden="true" /> Add a variant
            </Button>
          </div>
        </fieldset>
        <Field label="Base branch">
          {(props) =>
            branches && branches.length > 0 ? (
              <Select {...props} value={base} onChange={(e) => setBase(e.target.value)}>
                {branches.map((b) => (
                  <option key={b} value={b}>
                    {b}
                  </option>
                ))}
              </Select>
            ) : (
              <Input {...props} value={base} onChange={(e) => setBase(e.target.value)} />
            )
          }
        </Field>
        <Field label="Check" hint="Runs in each worktree when its run ends.">
          {(props) => (
            <Select {...props} value={check} onChange={(e) => setCheck(e.target.value)}>
              <option value="">No check</option>
              {actions.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </Select>
          )}
        </Field>
        <Field label="Approvals">
          {(props) => (
            <Select
              {...props}
              value={approval}
              onChange={(e) => setApproval(e.target.value as RunGroupChoices['approvalMode'])}
            >
              <option value="supervised">Supervised: ask before tools run</option>
              <option value="full-access">Full access</option>
            </Select>
          )}
        </Field>
        {error && (
          <p role="alert" className="m-0 text-[0.84rem] text-danger">
            {error}
          </p>
        )}
      </div>
    </Dialog>
  );
}
