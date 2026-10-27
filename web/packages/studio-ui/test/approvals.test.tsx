// Inline approval and question cards (SD1 Task 18; §53 §8.1).
import type { UserInputQuestion } from '@loams/agentd-client';
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import {
  approvalDetail,
  buildPendingUserInputAnswers,
  PendingApprovalCard,
  QuestionCard,
} from '../src/approvals';
import type { PendingApproval } from '../src/chat';

afterEach(cleanup);

const approval: PendingApproval = {
  id: 'call-7',
  chatId: 'chat-1',
  turnId: 'msg-u1',
  tool: 'Bash',
  args: { command: 'rm -rf build/ <img src=x onerror=alert(1)>' },
  risk: 'destructive',
  createdAtMs: 1,
};

describe('approval_card_shortcuts', () => {
  it('Enter allows once, Shift+Enter always, Esc denies', async () => {
    const onResolve = vi.fn(async () => undefined);
    const { getByRole } = render(
      <PendingApprovalCard approval={approval} autoFocus onResolve={onResolve} />,
    );
    const card = getByRole('region', { name: 'Command approval' });
    expect(document.activeElement).toBe(card);
    for (const [init, decision] of [
      [{ key: 'Enter' }, 'once'],
      [{ key: 'Enter', shiftKey: true }, 'always'],
      [{ key: 'Escape' }, 'deny'],
    ] as const) {
      onResolve.mockClear();
      await act(async () => {
        fireEvent.keyDown(card, init);
      });
      expect(onResolve).toHaveBeenCalledWith(approval, decision);
    }
  });

  it('ignores keys meant for a button inside the card, and the buttons decide too', async () => {
    const onResolve = vi.fn(async () => undefined);
    const { getByRole } = render(<PendingApprovalCard approval={approval} onResolve={onResolve} />);
    const always = getByRole('button', { name: /Always for this thread/ });
    fireEvent.keyDown(always, { key: 'Escape' });
    expect(onResolve).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(always);
    });
    expect(onResolve).toHaveBeenCalledWith(approval, 'always');
  });

  it('shows the request as text and surfaces a failure', async () => {
    const onResolve = vi.fn(async () => {
      throw new Error('approval_not_pending');
    });
    const { container, getByRole, findByRole } = render(
      <PendingApprovalCard approval={approval} onResolve={onResolve} />,
    );
    expect(container.querySelector('img')).toBeNull();
    expect(getByRole('region').textContent).toContain('rm -rf build/ <img src=x');
    expect(approvalDetail({ tool: 'X', args: { a: 1 } })).toBe('{\n  "a": 1\n}');
    fireEvent.click(getByRole('button', { name: /Deny/ }));
    expect((await findByRole('alert')).textContent).toBe('approval_not_pending');
  });
});

const questions: UserInputQuestion[] = [
  {
    id: 'q-1',
    header: 'Branch',
    question: 'Which branch?',
    options: ['main', 'dev'],
    multiSelect: false,
    multiline: false,
  },
  {
    id: 'q-2',
    header: 'Checks',
    question: 'Which checks?',
    options: ['lint', 'test', 'build'],
    multiSelect: true,
    multiline: false,
  },
  {
    id: 'q-3',
    header: 'Note',
    question: 'Anything else?',
    options: [],
    multiSelect: false,
    multiline: false,
    prefill: 'Ship it',
  },
];

describe('question_card_answers', () => {
  it('walks the questions and sends every answer once', async () => {
    const onAnswer = vi.fn(async () => undefined);
    const { getByRole, getByText, getByLabelText } = render(
      <QuestionCard requestId="req-1" questions={questions} resolved={false} onAnswer={onAnswer} />,
    );
    getByText('Which branch?');
    // A single-select option advances on its own.
    fireEvent.click(getByRole('button', { name: /dev/ }));
    await waitFor(() => getByText('Which checks?'));
    // Number keys pick options while focus is in the card; multi-select toggles.
    const card = getByRole('group', { name: 'Question: Checks' });
    fireEvent.keyDown(card, { key: '1' });
    fireEvent.keyDown(card, { key: '3' });
    fireEvent.click(getByRole('button', { name: 'Next' }));
    getByText('Anything else?');
    // Free text, prefilled.
    expect((getByLabelText('Answer') as HTMLInputElement).value).toBe('Ship it');
    expect(onAnswer).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(getByRole('button', { name: 'Send answer' }));
    });
    expect(onAnswer).toHaveBeenCalledTimes(1);
    expect(onAnswer).toHaveBeenCalledWith('req-1', [
      { questionId: 'q-1', labels: ['dev'] },
      { questionId: 'q-2', labels: ['lint', 'build'] },
      { questionId: 'q-3', labels: ['Ship it'] },
    ]);
  });

  it('typed text answers a question with options', () => {
    expect(
      buildPendingUserInputAnswers(questions.slice(0, 1), {
        'q-1': { customAnswer: '  feature/x ' },
      }),
    ).toEqual([{ questionId: 'q-1', labels: ['feature/x'] }]);
    expect(
      buildPendingUserInputAnswers(questions.slice(0, 2), {
        'q-1': { selectedOptionValues: ['main'] },
      }),
    ).toBeNull();
  });

  it('shows an answered question as answered, with no inputs', () => {
    const { container, getByText } = render(
      <QuestionCard requestId="req-1" questions={questions} resolved onAnswer={vi.fn()} />,
    );
    getByText('Branch: answered');
    expect(container.querySelector('button, input')).toBeNull();
  });
});
