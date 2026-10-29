// Mastra's feature telemetry sends events to PostHog unless MASTRA_TELEMETRY_DISABLED is
// set (read from @mastra/core 1.78.0's dist, 2026-10-23). The factory host is local
// software on ground the user owns: it phones nobody. This module is imported first by
// every entry, and Mastra reads the variable when it would send, not at import.
// A person who wants Mastra's telemetry sets MASTRA_TELEMETRY_DISABLED=0 themselves.
if (process.env.MASTRA_TELEMETRY_DISABLED === undefined) {
  process.env.MASTRA_TELEMETRY_DISABLED = '1';
}

export const TELEMETRY_DISABLED = process.env.MASTRA_TELEMETRY_DISABLED !== '0';
