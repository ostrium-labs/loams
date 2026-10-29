// The bundled plugins: code by package name, and every manifest the catalog
// may name (first-party and third-party). Vite splits each dynamic import
// into its own chunk. Per-plugin ESM bundles with an import map and SRI
// replace this table in AP1a Task 2 without changing the host's API.

import type { ModuleTable } from '@loams/console-host';
import helloPkg from '@loams/example-plugin-hello/package.json';
import accountPkg from '@loams/plugin-account/package.json';
import agentsPkg from '@loams/plugin-agents/package.json';
import approvalsPkg from '@loams/plugin-approvals/package.json';
import auditPkg from '@loams/plugin-audit/package.json';
import deploymentsPkg from '@loams/plugin-deployments/package.json';
import identityPkg from '@loams/plugin-identity/package.json';
import marketplacePkg from '@loams/plugin-marketplace/package.json';
import namespacesPkg from '@loams/plugin-namespaces/package.json';
import orgPkg from '@loams/plugin-org/package.json';
import projectsPkg from '@loams/plugin-projects/package.json';
import rpcPkg from '@loams/plugin-rpc/package.json';
import shellPkg from '@loams/plugin-shell/package.json';
import stackStatusPkg from '@loams/plugin-stack-status/package.json';
import studioPkg from '@loams/plugin-studio/package.json';

export const modules: ModuleTable = {
  '@loams/plugin-shell': () => import('@loams/plugin-shell'),
  '@loams/plugin-rpc': () => import('@loams/plugin-rpc'),
  '@loams/plugin-identity': () => import('@loams/plugin-identity'),
  '@loams/plugin-stack-status': () => import('@loams/plugin-stack-status'),
  '@loams/plugin-namespaces': () => import('@loams/plugin-namespaces'),
  '@loams/plugin-approvals': () => import('@loams/plugin-approvals'),
  '@loams/plugin-deployments': () => import('@loams/plugin-deployments'),
  // The console's own pages on loams.auth.v1 and loams.admin.v1 (API2 Task 10).
  '@loams/plugin-account': () => import('@loams/plugin-account'),
  '@loams/plugin-org': () => import('@loams/plugin-org'),
  '@loams/plugin-projects': () => import('@loams/plugin-projects'),
  '@loams/plugin-agents': () => import('@loams/plugin-agents'),
  '@loams/plugin-audit': () => import('@loams/plugin-audit'),
  '@loams/plugin-studio': () => import('@loams/plugin-studio'),
  // The Seed Exchange (MK1 Task 9).
  '@loams/plugin-marketplace': () => import('@loams/plugin-marketplace'),
};

export const manifests: unknown[] = [
  shellPkg,
  rpcPkg,
  identityPkg,
  stackStatusPkg,
  namespacesPkg,
  approvalsPkg,
  deploymentsPkg,
  accountPkg,
  orgPkg,
  projectsPkg,
  agentsPkg,
  auditPkg,
  studioPkg,
  marketplacePkg,
  helloPkg,
];

/** Third-party scripts, served beside the console (see vite.config.ts). */
export function sandboxScripts(base: string): Record<string, string> {
  return { '@loams/example-plugin-hello': `${base}plugins/hello/client.js` };
}
