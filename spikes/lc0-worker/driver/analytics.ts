// Reads Workers and Durable Object analytics (GraphQL) for the measurement window.
//   bash -c 'set -a; . ~/cloudflare-lc0.env; set +a; node driver/analytics.ts <from ISO> <to ISO> <out.json>'
// Logs "POST /client/v4/graphql" to $LC0_CF_LOG; never prints the token or account id.
import { appendFileSync, writeFileSync } from "node:fs";

const [from, to, outPath] = process.argv.slice(2);
const acct = process.env.CLOUDFLARE_ACCOUNT_ID!, token = process.env.CLOUDFLARE_API_TOKEN!;
async function gql(label: string, query: string) {
  if (process.env.LC0_CF_LOG) appendFileSync(process.env.LC0_CF_LOG, `POST /client/v4/graphql (${label})\n`);
  const r = await fetch("https://api.cloudflare.com/client/v4/graphql", {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify({ query, variables: { acct, from, to } }),
  });
  const j: any = await r.json();
  if (j.errors) throw new Error(JSON.stringify(j.errors).replaceAll(acct, ":acct"));
  return j.data.viewer.accounts[0];
}
const filt = `filter: { datetime_geq: $from, datetime_leq: $to }`;
const doPeriodic = await gql("DO periodic", `query($acct: string!, $from: Time!, $to: Time!) { viewer { accounts(filter: {accountTag: $acct}) {
  durableObjectsPeriodicGroups(limit: 1000, ${filt}) { dimensions { namespaceId objectId }
    sum { activeTime cpuTime duration inboundWebsocketMsgCount outboundWebsocketMsgCount rowsRead rowsWritten subrequests exceededCpuErrors } } } } }`);
const doInv = await gql("DO invocations", `query($acct: string!, $from: Time!, $to: Time!) { viewer { accounts(filter: {accountTag: $acct}) {
  durableObjectsInvocationsAdaptiveGroups(limit: 1000, ${filt}) { dimensions { namespaceId objectId type scriptName }
    sum { requests wallTime errors } } } } }`);
const workers = await gql("Workers invocations", `query($acct: string!, $from: Time!, $to: Time!) { viewer { accounts(filter: {accountTag: $acct}) {
  workersInvocationsAdaptive(limit: 100, filter: { datetime_geq: $from, datetime_leq: $to, scriptName: "loams-cloud-lc0" }) {
    dimensions { scriptName status } sum { requests errors subrequests cpuTimeUs wallTime } quantiles { cpuTimeP50 cpuTimeP99 } } } } }`);
const out = { from, to, doPeriodic: doPeriodic.durableObjectsPeriodicGroups, doInvocations: doInv.durableObjectsInvocationsAdaptiveGroups, workers: workers.workersInvocationsAdaptive };
writeFileSync(outPath, JSON.stringify(out, null, 2));
console.log(`periodic groups ${out.doPeriodic.length}, invocation groups ${out.doInvocations.length}, worker groups ${out.workers.length}`);
