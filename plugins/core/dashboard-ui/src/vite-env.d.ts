/// <reference types="vite/client" />

// Side-effect CSS imports (`import "./styles.css"`) need an ambient module
// declaration. Without it, a type-aware checker reports TS2882 on every one of
// them. `vite/client` provides the `*.css` module declarations, and declaring
// them here is also what makes the class-name imports in WidgetCard typecheck.
