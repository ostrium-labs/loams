// @loams/desktop-ui: components shared by the desktop's local-stack pages.

export { isWrite, type SqlDialect, stripSql } from '@loams/desktop/sql-lex';
export { ConnectPanel, clientCommand, type Dialect } from './connect-panel.js';
export { PageHead } from './page-head.js';
export { MAX_CELL_CHARS, MAX_ROWS, SqlConsole } from './sql-console.js';
export { RUNTIME_LINKS, StackCard } from './stack-card.js';
export { type TabDef, Tabs } from './tabs.js';
export { useStack } from './use-stack.js';
