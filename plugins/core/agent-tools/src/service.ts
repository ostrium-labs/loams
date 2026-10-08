import { Context, Service } from "cordis";
import { v4 as uuidv4 } from "uuid";
import type { Widget } from "@loams-core/types";
// Side-effect imports: augment cordis Context with every key this service injects.
// Re-declaring them locally would conflict with the owning packages.
import "@loams-core/store";
import "@loams-core/dashboards";
import "@loams-core/bi";
import "@loams-core/charts";
import "@loams-core/chart-specs";

export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: object;
  handler: (args: any) => Promise<any>;
}

declare module "cordis" {
  interface Context {
    agentTools: AgentToolsService;
  }
}

export class AgentToolsService extends Service {
  static inject = ["dashboard", "data", "render", "bi", "chartSpecs", "store"];

  constructor(ctx: Context) {
    super(ctx, "agentTools");
  }

  getToolDefinitions(): ToolDefinition[] {
    return [
      {
        name: "list_datasets",
        description: "List datasets available in the BI backend",
        inputSchema: {
          type: "object",
          properties: {},
        },
        handler: async () => {
          return await this.ctx.bi.listDatasets();
        },
      },
      {
        name: "describe_dataset",
        description: "Describe a dataset by ID",
        inputSchema: {
          type: "object",
          properties: {
            id: { type: "number" },
          },
          required: ["id"],
        },
        handler: async ({ id }) => {
          return await this.ctx.bi.describeDataset(id);
        },
      },
      {
        name: "get_dashboard",
        description: "Get a dashboard by ID",
        inputSchema: {
          type: "object",
          properties: {
            id: { type: "string" },
          },
          required: ["id"],
        },
        handler: async ({ id }) => {
          return await this.ctx.dashboard.load(id);
        },
      },
      {
        name: "list_dashboards",
        description: "List all dashboards",
        inputSchema: {
          type: "object",
          properties: {},
        },
        handler: async () => {
          return await this.ctx.store.listDashboards();
        },
      },
      {
        name: "create_dashboard",
        description: "Create a new dashboard",
        inputSchema: {
          type: "object",
          properties: {
            title: { type: "string" },
            spec: { type: "object" },
          },
          required: ["title"],
        },
        handler: async ({ title, spec }) => {
          return await this.ctx.dashboard.create(title, spec);
        },
      },
      {
        name: "patch_dashboard",
        description: "Apply JSON patch to a dashboard",
        inputSchema: {
          type: "object",
          properties: {
            id: { type: "string" },
            baseVersion: { type: "number" },
            ops: {
              type: "array",
              items: { type: "object" },
            },
          },
          required: ["id", "baseVersion", "ops"],
        },
        handler: async ({ id, baseVersion, ops }) => {
          return await this.ctx.dashboard.patch(id, baseVersion, ops, "agent");
        },
      },
      {
        name: "add_widget",
        description: "Add a widget to a dashboard",
        inputSchema: {
          type: "object",
          properties: {
            dashboardId: { type: "string" },
            widget: { type: "object" },
            position: { type: "object" },
          },
          required: ["dashboardId", "widget"],
        },
        handler: async ({ dashboardId, widget, position }) => {
          return await this.ctx.dashboard.addWidget(dashboardId, widget, position);
        },
      },
      {
        name: "add_chart_spec_widget",
        description: "Create and add a chart-specs widget to a dashboard",
        inputSchema: {
          type: "object",
          properties: {
            dashboardId: { type: "string" },
            datasetId: { type: "number" },
            chartSpec: { type: "object" },
            position: { type: "object" },
          },
          required: ["dashboardId", "datasetId", "chartSpec"],
        },
        handler: async ({ dashboardId, datasetId, chartSpec, position }) => {
          const validation = await this.ctx.chartSpecs.validate(chartSpec);
          if (!validation.valid || validation.spec === undefined) {
            throw new Error(`Invalid chart spec: ${validation.errors.join("; ")}`);
          }
          const widget: Widget = {
            id: uuidv4(),
            type: "chart",
            data: {
              source: "bi",
              datasetId,
            },
            chartSpec: validation.spec,
          };
          return await this.ctx.dashboard.addWidget(dashboardId, widget, position);
        },
      },
      {
        name: "remove_widget",
        description: "Remove a widget from a dashboard",
        inputSchema: {
          type: "object",
          properties: {
            dashboardId: { type: "string" },
            widgetId: { type: "string" },
          },
          required: ["dashboardId", "widgetId"],
        },
        handler: async ({ dashboardId, widgetId }) => {
          return await this.ctx.dashboard.removeWidget(dashboardId, widgetId);
        },
      },
      {
        name: "validate_spec",
        description: "Validate a dashboard specification",
        inputSchema: {
          type: "object",
          properties: {
            spec: { type: "object" },
          },
          required: ["spec"],
        },
        handler: async ({ spec }) => {
          // Assuming dashboard.validate() exists, or validate against DashboardSpecSchema
          return await this.ctx.dashboard.validate(spec);
        },
      },
      {
        name: "preview_widget",
        description: "Preview a widget, returning chart options and sample data",
        inputSchema: {
          type: "object",
          properties: {
            widget: { type: "object" },
          },
          required: ["widget"],
        },
        handler: async ({ widget }) => {
          // Assumes render.previewWidget exists
          return await this.ctx.render.previewWidget(widget);
        },
      },
    ];
  }
}
