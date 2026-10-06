/**
 * `@loams-plugins/plugin-flow-render` -- the renderer for `graph` widgets.
 *
 * A sibling of `plugin-echarts-render`, not an extension of it. See the module
 * comments in `compiler.ts` and `service.ts` for why: React Flow's terminal
 * value is a `nodes`/`edges` pair, not an ECharts option object.
 */
export * from "./compiler.js";
export * from "./theme.js";
export * from "./service.js";
