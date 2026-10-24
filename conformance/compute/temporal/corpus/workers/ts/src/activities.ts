// The corpus's TypeScript activities (DX0 Task 4), named as the Go ones.
import { Context } from '@temporalio/activity';
import { ApplicationFailure } from '@temporalio/common';

export async function greet(name: string): Promise<string> {
  return `hello ${name}`;
}

export async function heartbeat_thrice(): Promise<number> {
  for (let i = 1; i <= 3; i++) Context.current().heartbeat(i);
  return 3;
}

export async function fail_twice(): Promise<number> {
  const attempt = Context.current().info.attempt;
  if (attempt < 3) throw ApplicationFailure.retryable(`attempt ${attempt} fails`, 'Transient');
  return attempt;
}

export async function sleep_for(ms: number): Promise<void> {
  await Context.current().sleep(ms);
}

export async function large(n: number): Promise<string> {
  return 'x'.repeat(n);
}
