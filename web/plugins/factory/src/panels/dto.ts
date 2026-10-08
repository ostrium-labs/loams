// The DTOs the main-process ops return (apps/desktop-electron/src/main/factory/ops.ts).
// Every field is optional: the ops project whatever the upstream sent.

export interface ForgejoRepo {
  fullName?: string;
  description?: string;
  stars?: number;
  forks?: number;
  openIssues?: number;
  updatedAt?: string;
  url?: string;
}
export interface ForgejoIssue {
  id?: string;
  title?: string;
  state?: string;
  url?: string;
  updatedAt?: string;
}
export interface ZulipStream {
  id?: number;
  name?: string;
  description?: string;
  private?: boolean;
}
export interface ZulipMessage {
  id?: number;
  sender?: string;
  timestamp?: number;
  topic?: string;
  content?: string;
}
export interface PlaneIssue {
  id?: string;
  title?: string;
  priority?: string;
  updatedAt?: string;
}
export interface GlitchtipOrg {
  slug?: string;
  name?: string;
}
export interface GlitchtipIssue {
  id?: string;
  title?: string;
  level?: string;
  count?: string | number;
  lastSeen?: string;
  permalink?: string;
}
export interface OpenPanelInsights {
  summary: Record<string, number>;
  series: ({ date?: string } & Record<string, number | string | undefined>)[];
  topPages: { path?: string; sessions?: number; pageviews?: number }[];
}
export type MatomoVisit = { date?: string } & Record<string, number | string | undefined>;
export interface MatomoPage {
  label?: string;
  hits?: number;
  visits?: number;
}
export interface LangfuseTrace {
  id?: string;
  traceId?: string;
  name?: string;
  startTime?: string;
  level?: string;
  totalCost?: number;
}
export interface LangfuseDaily {
  available: boolean;
  days: ({ date?: string } & Record<string, number | string | undefined>)[];
}
