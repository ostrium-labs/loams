import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ErrorCode,
  ListToolsRequestSchema,
  McpError,
} from "@modelcontextprotocol/sdk/types.js";
import { Context } from "cordis";
import { AgentToolsService } from "./service.js";

export function startMCPServer(ctx: Context) {
  const server = new Server(
    {
      name: "cdp-agent-tools",
      version: "0.1.0",
    },
    {
      capabilities: {
        tools: {},
      },
    },
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => {
    const toolsService = ctx.get("agentTools") as AgentToolsService;
    if (!toolsService) {
      throw new Error("agentTools service not available");
    }

    const defs = toolsService.getToolDefinitions();
    return {
      tools: defs.map((def) => ({
        name: def.name,
        description: def.description,
        inputSchema: def.inputSchema,
      })),
    };
  });

  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const toolsService = ctx.get("agentTools") as AgentToolsService;
    if (!toolsService) {
      throw new McpError(ErrorCode.InternalError, "agentTools service not available");
    }

    const defs = toolsService.getToolDefinitions();
    const tool = defs.find((t) => t.name === request.params.name);

    if (!tool) {
      throw new McpError(ErrorCode.MethodNotFound, `Unknown tool: ${request.params.name}`);
    }

    try {
      const result = await tool.handler(request.params.arguments || {});
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(result, null, 2),
          },
        ],
      };
    } catch (error: any) {
      return {
        isError: true,
        content: [
          {
            type: "text",
            text: error?.message || String(error),
          },
        ],
      };
    }
  });

  const transport = new StdioServerTransport();
  server.connect(transport).catch((err: unknown) => {
    const message = err instanceof Error ? err.stack || err.message : String(err);
    process.stderr.write(`MCP Server error: ${message}\n`);
  });

  return server;
}
