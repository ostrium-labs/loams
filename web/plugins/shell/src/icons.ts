import {
  Boxes,
  Circle,
  Cloud,
  Database,
  Factory,
  Layers,
  LayoutDashboard,
  type LucideIcon,
  Network,
  Plug,
  Radio,
  Settings,
  ShieldCheck,
  Table2,
  Waypoints,
  Workflow,
} from 'lucide-react';

/** Icon names a `shell.nav.section` entry may use (plugins pass a name, never a component). */
const ICONS: Record<string, LucideIcon> = {
  overview: LayoutDashboard,
  data: Table2,
  postgres: Database,
  wesql: Layers,
  live: Radio,
  durable: Workflow,
  streams: Waypoints,
  connectors: Plug,
  graph: Network,
  factory: Factory,
  cloud: Cloud,
  settings: Settings,
  approvals: ShieldCheck,
  namespaces: Boxes,
};

export const ICON_NAMES = Object.keys(ICONS);

export function iconFor(name: string | undefined): LucideIcon {
  return (name && ICONS[name]) || Circle;
}
