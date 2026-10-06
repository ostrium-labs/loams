import { Context, Service } from "cordis";
import jsonpatch from "fast-json-patch";
type Operation = any;
const applyPatch = (jsonpatch as any).applyPatch || (jsonpatch as any).default?.applyPatch;
import { v4 as uuidv4 } from "uuid";
import { DashboardSpec, DashboardSpecSchema, Widget } from "@loams-plugins/types";
// Loaded for its side effect: augments cordis Context with the real `store` key.
// Re-declaring `store` locally would conflict with plugin-store's own declaration.
import "@loams-plugins/plugin-store";

declare module "cordis" {
  interface Context {
    dashboard: DashboardSpecService;
  }
}

export type DashboardValidationResult =
  | { valid: true; errors: string[]; spec: DashboardSpec }
  | { valid: false; errors: string[]; spec: undefined };

export interface DashboardEvents {
  "dashboard/loaded"(spec: DashboardSpec): void;
  "dashboard/created"(spec: DashboardSpec): void;
  "dashboard/patched"(spec: DashboardSpec, ops: Operation[], actor?: string): void;
}

declare module "cordis" {
  interface Events extends DashboardEvents {}
}

export class DashboardSpecService extends Service {
  static inject = ["store"];

  private _spec: DashboardSpec | null = null;
  private _undoStack: Operation[][] = [];
  private _redoStack: Operation[][] = [];

  constructor(ctx: Context) {
    super(ctx, "dashboard");
  }

  get spec(): DashboardSpec | null {
    return this._spec;
  }

  validate(spec: unknown): DashboardValidationResult {
    const result = DashboardSpecSchema.safeParse(spec);
    if (result.success) {
      return { valid: true, errors: [], spec: result.data };
    }
    const errors = result.error.issues.map((issue) => {
      const path = issue.path.length > 0 ? issue.path.join(".") : "(root)";
      return `${path}: ${issue.message}`;
    });
    return { valid: false, errors, spec: undefined };
  }

  async load(id: string): Promise<DashboardSpec> {
    const data = await this.ctx.store.getDashboard(id);
    const spec = DashboardSpecSchema.parse(data);
    this._spec = spec;
    this.ctx.emit("dashboard/loaded", spec);
    return spec;
  }

  async create(title: string, partialSpec?: Partial<DashboardSpec>): Promise<DashboardSpec> {
    const spec: DashboardSpec = DashboardSpecSchema.parse({
      id: uuidv4(),
      version: 0,
      title,
      params: [],
      layout: [],
      widgets: {},
      ...partialSpec,
    });

    await this.ctx.store.saveDashboard(spec);
    this._spec = spec;
    this.ctx.emit("dashboard/created", spec);
    return spec;
  }

  async patch(
    id: string,
    baseVersion: number,
    ops: Operation[],
    actor?: string,
  ): Promise<DashboardSpec> {
    let spec = this._spec;
    if (!spec || spec.id !== id) {
      spec = await this.load(id);
    }

    if (spec.version !== baseVersion) {
      throw new Error(`Version conflict: expected ${baseVersion}, got ${spec.version}`);
    }

    const newDocument = JSON.parse(JSON.stringify(spec));
    applyPatch(newDocument, ops);
    newDocument.version += 1;

    const validated = DashboardSpecSchema.parse(newDocument);
    await this.ctx.store.saveDashboard(validated);

    this._spec = validated;
    this._undoStack.push(ops);
    this._redoStack = [];

    this.ctx.emit("dashboard/patched", validated, ops, actor);
    return validated;
  }

  async addWidget(
    id: string,
    widget: Widget,
    position?: { x: number; y: number; w: number; h: number },
    actor?: string,
  ): Promise<DashboardSpec> {
    let spec = this._spec;
    if (!spec || spec.id !== id) {
      spec = await this.load(id);
    }

    const ops: Operation[] = [{ op: "add", path: `/widgets/${widget.id}`, value: widget }];

    const maxY = spec.layout.reduce((max, item) => Math.max(max, item.y + item.h), 0);
    const safePos = {
      x:
        typeof position?.x === "number" && Number.isFinite(position.x)
          ? Math.max(0, Math.floor(position.x))
          : 0,
      y:
        typeof position?.y === "number" && Number.isFinite(position.y)
          ? Math.max(0, Math.floor(position.y))
          : maxY,
      w:
        typeof position?.w === "number" && Number.isFinite(position.w)
          ? Math.min(12, Math.max(1, Math.floor(position.w)))
          : 6,
      h:
        typeof position?.h === "number" && Number.isFinite(position.h)
          ? Math.max(1, Math.floor(position.h))
          : 4,
    };

    ops.push({
      op: "add",
      path: "/layout/-",
      value: { id: widget.id, ...safePos },
    });

    return this.patch(id, spec.version, ops, actor);
  }

  async removeWidget(
    dashboardId: string,
    widgetId: string,
    actor?: string,
  ): Promise<DashboardSpec> {
    let spec = this._spec;
    if (!spec || spec.id !== dashboardId) {
      spec = await this.load(dashboardId);
    }

    const ops: Operation[] = [];
    if (spec.widgets && spec.widgets[widgetId] !== undefined) {
      const escapedId = widgetId.replace(/~/g, "~0").replace(/\//g, "~1");
      ops.push({ op: "remove", path: `/widgets/${escapedId}` });
    }

    const layoutIndex = spec.layout ? spec.layout.findIndex((item) => item.id === widgetId) : -1;
    if (layoutIndex !== -1) {
      ops.push({ op: "remove", path: `/layout/${layoutIndex}` });
    }

    if (ops.length === 0) {
      return spec;
    }

    return this.patch(dashboardId, spec.version, ops, actor);
  }

  async moveWidget(
    dashboardId: string,
    widgetId: string,
    position: { x: number; y: number; w: number; h: number },
    actor?: string,
  ): Promise<DashboardSpec> {
    let spec = this._spec;
    if (!spec || spec.id !== dashboardId) {
      spec = await this.load(dashboardId);
    }

    const layoutIndex = spec.layout.findIndex((item) => item.id === widgetId);
    if (layoutIndex === -1) {
      throw new Error(`Widget ${widgetId} not found in layout`);
    }

    const ops: Operation[] = [
      { op: "replace", path: `/layout/${layoutIndex}/x`, value: position.x },
      { op: "replace", path: `/layout/${layoutIndex}/y`, value: position.y },
      { op: "replace", path: `/layout/${layoutIndex}/w`, value: position.w },
      { op: "replace", path: `/layout/${layoutIndex}/h`, value: position.h },
    ];

    return this.patch(dashboardId, spec.version, ops, actor);
  }

  async undo() {
    // Placeholder
  }

  async redo() {
    // Placeholder
  }
}
