// Types of `loams:agent`, the agent profile's host module (SPIKE, MS1
// Task 1). The worker's prelude provides the values.
declare module "loams:agent" {
  export interface AcpConnection {
    send(message: unknown): void;
  }
  export const acp: {
    connect(handler: (message: any) => void | Promise<void>): AcpConnection;
    stream(): { input: ReadableStream<Uint8Array>; output: WritableStream<Uint8Array> };
  };
  type ModelCall = {
    (request: unknown): Promise<any>;
    (request: unknown, options: { stream: true }): AsyncIterable<any> & { result: Promise<any> };
  };
  export const model: { chat: ModelCall; responses: ModelCall; messages: ModelCall };
  export const mcp: {
    call(server: string, tool: string, args?: Record<string, unknown>): Promise<any>;
    list(server: string): Promise<{ tools: { name: string; description?: string }[] }>;
  };
  export const loams: { token(): Promise<string>; config(): Record<string, string> };
}
