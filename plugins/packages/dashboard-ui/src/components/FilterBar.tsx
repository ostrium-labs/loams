import React from "react";
import { Label, cn } from "@loams-plugins/core/ui";

interface FilterBarProps {
  params: Array<{
    name: string;
    type: string;
    default?: unknown;
    datasetId?: number;
    column?: string;
  }>;
  paramValues: Record<string, unknown>;
  onChange: (name: string, value: unknown) => void;
}

/*
 * Sample predefined choices for filters.
 *
 * These stand in for the dataset's real distinct values. Keeping them in one
 * object means adding a filter is a single entry here rather than a new branch
 * in the render below.
 */
const FILTER_OPTIONS: Record<string, string[]> = {
  region: ["All", "North America", "Europe", "Asia Pacific"],
  country: ["All", "United States", "Germany"],
  category: ["All", "Enterprise Cloud", "Developer Tools", "Security & Auth", "Data & Analytics"],
  status: ["All", "Healthy", "Warning", "Critical"],
};

const SELECT = cn(
  "cursor-pointer rounded border border-line bg-card px-2 py-1 font-sans text-[0.78rem] text-ink",
  "hover:border-ink-subtle focus-visible:border-ink-subtle",
);

export const FilterBar: React.FC<FilterBarProps> = ({ params, paramValues, onChange }) => {
  if (!params || params.length === 0) return null;

  return (
    <div className="flex items-center justify-between gap-4 border-b border-line bg-card px-8 py-[0.55rem]">
      <span className="text-[0.78rem] font-semibold text-ink-body">Global Filters:</span>
      {params.map((p) => {
        const val = paramValues[p.name] !== undefined ? paramValues[p.name] : p.default || "All";
        const options = FILTER_OPTIONS[p.name] || ["All", "Option A", "Option B"];

        return (
          <div key={p.name} className="flex items-center gap-[0.6rem]">
            {/*
             * The T3 `Label` renders through Base UI's `useRender`, so it can
             * carry a `render` prop. Not used here: wrapping the control it
             * names is the accessibility-correct thing to do, and `htmlFor`
             * achieves it without the indirection.
             */}
            <Label
              htmlFor={`filter-${p.name}`}
              className="text-[0.78rem] font-semibold text-ink-body"
            >
              {p.name}:
            </Label>
            <select
              id={`filter-${p.name}`}
              className={SELECT}
              value={String(val)}
              onChange={(e) => onChange(p.name, e.target.value === "All" ? "" : e.target.value)}
            >
              {options.map((opt) => (
                <option key={opt} value={opt}>
                  {opt}
                </option>
              ))}
            </select>
          </div>
        );
      })}
    </div>
  );
};
