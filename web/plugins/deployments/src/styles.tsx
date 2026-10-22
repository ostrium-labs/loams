// The page's own styles, scoped under `ld-` and drawn only from @loams/ui tokens, so
// light and dark follow the console's theme class. Every animation stops for
// prefers-reduced-motion, and also when the node says motion is off (data-motion).

const CSS = `
.ld-page { display: flex; flex-direction: column; gap: 16px; }
.ld-list { display: flex; flex-direction: column; gap: 16px; }
.ld-meta { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; color: var(--op-muted); font-size: 13px; margin: 0 0 8px; }
.ld-canvas { position: relative; width: 100%; border: 1px solid var(--op-rule-soft); background: var(--op-surface); }
.ld-canvas .react-flow { --xy-background-color: var(--op-surface); --xy-edge-stroke: var(--op-rule); }
.ld-canvas .react-flow__node { border-radius: 0; }
.ld-canvas .react-flow__node:focus-visible { outline: 2px solid var(--op-accent); outline-offset: 3px; }
.ld-node { box-sizing: border-box; width: 100%; height: 100%; padding: 8px 10px; display: flex; flex-direction: column; gap: 6px;
  background: var(--op-bg); color: var(--op-ink); border: 1px solid var(--op-rule); font-size: 12px; overflow: hidden; }
.ld-node[data-state="running"] { border-color: var(--op-accent); }
.ld-node[data-state="succeeded"] { border-color: var(--op-grow); }
.ld-node[data-state="failed"] { border-color: var(--op-danger); background: var(--op-danger-soft); }
.ld-node[data-state="pending"], .ld-node[data-state="skipped"] { color: var(--op-muted); }
.ld-node[data-branch] { border-style: dashed; }
.ld-node[data-selected] { box-shadow: 0 0 0 2px var(--op-accent-soft); }
.ld-node-head { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.ld-node-head strong { font-size: 13px; }
.ld-node-foot { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.ld-muted { color: var(--op-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ld-progress { height: 6px; background: var(--op-raised); overflow: hidden; }
.ld-progress-fill { display: block; height: 100%; background: var(--op-grow); transition: width 400ms var(--op-ease); }
.ld-node[data-state="running"] .ld-progress-fill { background: var(--op-accent); }
.ld-node[data-state="failed"] .ld-progress-fill { background: var(--op-danger); }
.ld-running { background-image: linear-gradient(90deg, transparent, var(--op-accent-soft), transparent);
  background-size: 200% 100%; animation: ld-sweep 1.4s linear infinite; }
@keyframes ld-sweep { from { background-position: 200% 0; } to { background-position: -200% 0; } }
.ld-node[data-motion="reduce"] .ld-progress-fill { transition: none; }
.ld-node[data-motion="reduce"] .ld-running { animation: none; background-image: none; }
.ld-link { background: none; border: 0; padding: 0; color: var(--op-accent); font: inherit; text-decoration: underline; cursor: pointer; }
.ld-link:focus-visible { outline: 2px solid var(--op-accent); outline-offset: 2px; }
.ld-keys { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; overflow: auto; }
.ld-keys li { color: var(--op-muted); }
.ld-keys li[data-kind="rotated"] code { color: var(--op-accent); }
.ld-keys li[data-kind="removed"] code { text-decoration: line-through; }
.ld-keys code { font-family: var(--loams-font-mono); font-size: 11px; color: var(--op-ink); } /* mono-ok: code */
.ld-logs { margin: 0; padding: 8px; background: var(--op-surface); font-family: var(--loams-font-mono); font-size: 12px; /* mono-ok: code */
  max-height: 320px; overflow: auto; list-style: none; }
.ld-logs li[data-level="error"] { color: var(--op-danger); }
.ld-charts { display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 16px; }
@media (prefers-reduced-motion: reduce) {
  .ld-progress-fill { transition: none; }
  .ld-running { animation: none; background-image: none; }
  .ld-canvas .react-flow__edge-path { animation: none !important; }
}
`;

export function DeployStyles() {
  return <style>{CSS}</style>;
}
