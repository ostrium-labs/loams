/**
 * Where the shell receives the dashboard's own component.
 *
 * The dashboard lives in `@loams-plugins/dashboard-ui`, which imports `@loams-plugins/core/ui` --
 * so core cannot import it back without a cycle. The composition happens
 * through this slot instead: the host renders `<CoreShell dashboard={App} />`
 * and the dashboard plugin page picks it up here.
 */
import React from "react";

export const DashboardSlotContext = React.createContext<React.ComponentType | null>(null);

export function useDashboardSlot(): React.ComponentType | null {
  return React.useContext(DashboardSlotContext);
}
