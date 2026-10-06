import React from "react";
import { Alert, AlertDescription, AlertTitle, Badge } from "@loams-plugins/core/ui";
import { InfoCircleIcon } from "./Icons";
import type { ThemeReportEntry } from "../theme/types";

interface ThemeReportPanelProps {
  report: ThemeReportEntry[];
  error: string | null;
  resolving: boolean;
}

/**
 * Surfaces flint's `report` and its invalid-theme error.
 *
 * flint deliberately does not swallow downgrades — "silent fallbacks are
 * indistinguishable from bugs" — so this panel does the same. The report is
 * informational and non-blocking: the theme IS applied, flint just had to
 * approximate something. The error state is separate and blocking, because an
 * unknown preset name means flint refused to resolve at all and nothing is
 * being themed; rendering the dashboard as if it were themed would be a lie.
 *
 * Both states are `Alert`: an error and a warning are exactly its variants, and
 * the one accent it needs is a leading icon plus a two-line slot.
 */
export const ThemeReportPanel: React.FC<ThemeReportPanelProps> = ({ report, error, resolving }) => {
  if (error) {
    return (
      <Alert variant="error">
        <AlertTitle>Theme could not be applied</AlertTitle>
        <AlertDescription>{error}</AlertDescription>
        <AlertDescription>
          Flint treats an unknown preset name as an error rather than quietly rendering unthemed, so
          the dashboard is left on its defaults and the problem is shown here instead.
        </AlertDescription>
      </Alert>
    );
  }

  if (resolving) {
    return (
      <Alert variant="info">
        <InfoCircleIcon />
        <AlertTitle>Resolving theme…</AlertTitle>
      </Alert>
    );
  }

  if (report.length === 0) return null;

  return (
    <Alert variant="warning">
      <InfoCircleIcon />
      <AlertTitle>
        {report.length} simplification{report.length === 1 ? "" : "s"} from Flint
      </AlertTitle>
      <AlertDescription>
        <p className="m-0">
          This theme resolved, but Flint had to approximate the following. Nothing is broken — it is
          telling you where it traded fidelity for a rule.
        </p>
        <ul className="m-0 flex list-none flex-col gap-[0.4rem]">
          {report.map((entry, i) => (
            <li
              key={`${entry.stage}-${entry.path}-${i}`}
              className="flex flex-wrap items-baseline gap-[0.4rem] text-[0.73rem]"
            >
              <Badge variant="outline" size="sm" className="uppercase">
                {entry.stage}
              </Badge>
              <code className="font-[Cascadia_Mono,ui-monospace,SFMono-Regular,Menlo,monospace] text-primary-ink">
                {entry.path}
              </code>
              <span className="min-w-32 flex-1 text-ink-body">{entry.message}</span>
            </li>
          ))}
        </ul>
      </AlertDescription>
    </Alert>
  );
};
