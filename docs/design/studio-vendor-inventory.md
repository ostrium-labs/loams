# Loams Studio: the T3 Code vendoring inventory

Status: **Proposed** · 2026-10-13. Appendix A of [§53 Loams Studio](53-studio-ade.md). Plan: [SD1](../plans/2026-10-13-sd1-studio-ade.md).

**As built (2026-10-15).** This inventory is the plan; what was actually taken is recorded per file in `web/packages/studio-ui/provenance.json` (104 files: 24 as-is, 79 adapted, 1 rewritten) and `web/packages/ui/provenance.json` (12 primitives), each row with its verdict, group and note. Where a task took, left or rewrote something differently from this list, its ruling in SD1's "Rulings made during execution" says why (for example T17-1, T18-13, T19-2, T21-1, T22-2, T26-6). The re-sync procedure is in [`web/packages/studio-ui/README.md`](../../web/packages/studio-ui/README.md).

This is the list of T3 Code files and modules that Studio takes, adapts, rewrites or skips. It was read from the source tree only: nothing in it was run, installed or built.

## Source

| | |
|---|---|
| Repository | `pingdotgg/t3code`, branch `main` |
| Commit | `6524a8d43795dc543f7cfb572388cdea3662ad48`, from the GitHub archive's zip comment. `main` as of 2026-10-13T07:16:38Z; `apps/web` is `@t3tools/web` 0.0.45 |
| Archive | `t3code.zip`, SHA-256 `b9ca31efad706d6828f3d9fb6b411027d5837901b800790d241eb672a74b7cbd` |
| Licence | MIT, "Copyright (c) 2026 T3 Tools Inc." The full text is in §53 §16, and becomes `web/packages/studio-ui/LICENSE.t3code` |

## How to read the table

- **Paths.** They are relative to `apps/web/src/` unless they start with `pkg` (`packages/<name>/src/`).
- **Lines.** Non-test lines, counted by line-counting scripts kept outside the source tree.
- **Effect/state/server deps.** What the file imports from T3 Code's runtime, and what replaces it in Loams. The abbreviations:
  - *atoms*: `@effect/atom-react`, `effect/reactivity`, `~/state/*` and the client-runtime state modules.
  - *contracts*: `@t3tools/contracts` types, which are derived from Effect Schema.
  - *zustand*: a client-only UI store.
- **What replaces them:**
  - **CQ**: connect-query hooks on `rpc.agentd.*` (`loams.agentd.v1`).
  - **AC**: `@loams/agentd-client` types and its transcript reducer.
  - **Props**: the data is passed in from the parent; no store.
  - **zod**: replaces `effect/Schema` decoding of persisted values.
  - **plain**: `effect` helpers (`Array`, `Option`, `DateTime`, `Equal`, `Order`) replaced by plain TypeScript or `Intl`.
  - **VL**: `@loams/ui` `VirtualList` (TanStack Virtual) replaces `@legendapp/list`.
  - **UI**: `@loams/ui` primitives (Base UI wrapped once) replace T3's `components/ui`.
  - **HR**: the shell's hash router (`router.page`, `router.navigate`) replaces TanStack Router.
- **Licences.** Third-party licences were checked on the npm registry on 2026-10-13 (`npm view <pkg> license`) against CONTRIBUTING rule 3 and `deny.toml`'s allow list. ✓ means allowed. ✗ means not allowed.
- **Verdicts:**
  - **as-is**: header, import paths and token classes only.
  - **adapt**: vendored, with its data layer and styling ported.
  - **rewrite**: ours, using the upstream file as the reference. It is credited in NOTICE when its structure follows upstream closely.
  - **skip**: not taken.
- **Target.** The path under `web/packages/studio-ui/src/` (abbreviated `sui/`), `web/plugins/studio/src/` (`ps/`) or `web/packages/ui/src/` (`ui/`), and the SD1 task.

## Totals (estimate)

| Verdict | Files (about) | Upstream lines (about) |
|---|---|---|
| as-is | 75 | 9 500 |
| adapt | 70 | 38 000 |
| rewrite (reference only) | 12 | 36 000 upstream → about 9 000 of ours |
| skip | everything else in `apps/web` (cloud, Clerk, auth, onboarding, usage, mobile device, PR review product, browser streaming, settings panels) and all of `apps/server`, `apps/desktop`, `apps/mobile`, `apps/marketing`, `packages/provider-*`, `packages/source-control-*`, `packages/effect-*`, `packages/ssh`, `packages/tailscale`, `packages/client-runtime/rpc` | about 330 000 in `apps/web` and over 900 000 in the rest |

Overall, Studio takes about 15% of `apps/web`'s non-test lines. Most of it is leaf components and pure logic; the containers are rewritten.

---

## (a) Thread list and sidebar → SD1 Task 17

| Upstream path | Purpose | Lines | Effect/atom/server deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/Sidebar.tsx | Sidebar: project groups, thread rows, pin, snooze, settle, reorder by drag | 5 656 | atoms, `effect/Schema`, about 15 state modules, five zustand stores → CQ (`WatchSpaces`, `WatchChats`, `WatchSessions`, `WatchRunGroups`), Props, zod | @dnd-kit/* (MIT ✓), @tanstack/react-router (MIT; → HR), lucide-react (ISC ✓) | rewrite | `ps/sidebar/Sidebar.tsx` |
| components/Sidebar.logic.ts | Pure grouping, sorting and visibility of rows | 1 457 | light `effect/reactivity`; client-runtime sort, inbox, settled, search; contracts → plain, AC types | @dnd-kit/sortable (MIT ✓) | adapt | `sui/thread-list/logic.ts` |
| components/Sidebar.drag.ts | Drag sensors and collision helpers | 266 | none | @dnd-kit/core, sortable (MIT ✓) | as-is | `sui/thread-list/drag.ts` |
| components/Sidebar.motion.ts, Sidebar.pointer.ts, Sidebar.snooze.ts | Motion, pointer and snooze helpers | 266 / 157 / 57 | none | — | as-is | `sui/thread-list/` |
| components/AppSidebarLayout.tsx | Sidebar, main and right-panel layout | 361 | atoms, `effect/Schema`, rightPanel and terminal UI stores → Props, zod, zustand | → HR | adapt | `ps/layout/StudioLayout.tsx` |
| components/sidebar/SidebarChrome, SidebarThreadHeader | Sidebar header chrome | about 500 of 1 915 | atoms (update state) → Props | lucide (ISC ✓) | adapt | `sui/thread-list/` |
| components/sidebar/* (update pills, release notes) | App update UI | about 1 400 | desktop update state | — | skip (Electron's updater owns updates) | — |
| components/LegacySidebar.tsx | The previous sidebar | 3 898 | — | — | skip | — |
| components/ThreadStatusIndicators.tsx | Badges per thread: running, approval, PR, terminal | 1 177 | `effect/reactivity`, atom registry, vcs, pull request and terminal state → Props from CQ | @base-ui/react (→ UI), lucide | adapt | `sui/thread-list/StatusIndicators.tsx` |
| components/threadActionMenu.logic.ts | Thread context menu model | 265 | client-runtime settled → plain | — | adapt | `sui/thread-list/actionMenu.ts` |
| hooks/useThreadActionMenu.ts, hooks/useThreadActions.ts | Archive, delete, rename and fork actions, with a worktree clean-up prompt | 377 + 1 046 | `effect` Cause, Schema and reactivity; orchestration commands → CQ mutations (`Mutate`, `ForkSideChat`, `DeleteWorktree`) | → HR | rewrite | `ps/sidebar/useThreadActions.ts` |
| sidebarProjectGrouping.ts | Groups projects by repository | 225 | client-runtime grouping (Effect-free) → AC types | — | adapt | `sui/thread-list/grouping.ts` |
| worktreeCleanup.ts | Finds orphaned worktrees on delete | 45 | types only → AC | — | as-is | `sui/thread-list/worktreeCleanup.ts` |
| lib/threadSort.ts, threadRoutes.ts, threadSelectionStore.ts | Sort key, route helpers, multi-select | 7 / 106 / 131 | zustand (selection) | zustand (MIT ✓) | as-is (routes → adapt to HR) | `sui/thread-list/` |
| pkg client-runtime/state/{threadSort, threadSettled, threadInbox, projectGrouping}.ts | Pure sorting, settled, inbox and grouping rules | 1 277 | `effect` Array, Order, DateTime → plain | — | adapt | `sui/thread-list/rules/` |

## (b) Chat view, timeline and markdown → SD1 Task 18

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/ChatView.tsx | The thread page: timeline, composer, panels, approvals, queue | 12 236 | atoms; `effect` Cause, DateTime, Schema, reactivity; about 30 state modules; zustand → CQ, AC, Props | @legendapp/list (→ VL), @tanstack/react-pacer (MIT ✓), → HR | rewrite | `ps/thread/ThreadPage.tsx` |
| components/ChatView.logic.ts | Derived view logic: worktree setup, attachments, preview | 1 347 | `effect` DateTime, Option, Schema; atom registry → plain, AC | — | adapt (selected functions) | `sui/chat/view.logic.ts` |
| components/chat/MessagesTimeline.tsx | Virtualised timeline: messages, work log, tool rows, per-turn file summaries, subagents | 5 941 | atoms, `effect/DateTime`, threads state, client-runtime work-log → AC reducer state as Props, VL | @legendapp/list (→ VL), @pierre/diffs (Apache-2.0 ✓), react-markdown (MIT ✓), remark-gfm (MIT ✓) | adapt (split into rows) | `sui/chat/Timeline.tsx` + `sui/chat/rows/*` |
| components/chat/MessagesTimeline.logic.ts | Turn items → timeline rows | 2 374 | `effect` DateTime, Equal; client-runtime shell, subagent, work-log; zustand → AC (`TranscriptUpdate` parts) | zustand | adapt (the key reuse target: retargeted from T3 turn items to agentd message parts) | `sui/chat/timeline.logic.ts` |
| session-logic.ts | Session and turn presentation: work entries, pending requests, checkpoints | 1 097 | `effect` DateTime, Equal; client-runtime checkpoints, requests, execution → AC, plain | zustand | adapt | `sui/chat/session.logic.ts` |
| components/ChatMarkdown.tsx | Markdown: code blocks (Shiki), file links, citations, images, mermaid | 3 483 | atoms, `effect` Cause and reactivity, preview, projects, assets and server state → Props; link resolution through `ReadWorkspaceFile` | react-markdown, hast-util-to-html (MIT ✓) | adapt, under §53 D919: **no raw HTML, no remote images**; images only as local blobs; http(s) links only | `sui/markdown/Markdown.tsx` |
| markdown-incremental.ts, markdown-links.ts, markdown-clipboard.ts | Streaming split, link rewriting, rich clipboard | 107 / 98 / 411 | none | unified (MIT ✓) | as-is | `sui/markdown/` |
| lib/syntaxHighlighting.ts, lib/incrementalHighlighting.ts | Shiki through `@pierre/diffs`' highlighter | 39 / 74 | none | @pierre/diffs (Apache-2.0 ✓), shiki (MIT ✓) | as-is | `sui/markdown/highlight.ts` |
| components/chat/HighlightedTokens.tsx, HighlightedCodeLines.tsx | Renders tokens as React spans | 204 / 48 | none | — | as-is (these satisfy D919) | `sui/markdown/` |
| components/chat/MermaidDiagram.tsx | Mermaid, sanitised | 217 | none | mermaid (MIT ✓), dompurify (MPL-2.0 OR Apache-2.0 ✓) | skip in SD1 (a sizeable dependency and an SVG injection surface; Q in Task 18) | — |
| components/chat/WorkLog.tsx, ShellCommandBlock.tsx, components/CommandBlock.tsx | Tool and command rows | 148 / 76 / 45 | client-runtime work-log (Effect-free) → AC tool parts | — | as-is | `sui/chat/rows/` |
| components/chat/V2LifecycleRow.tsx, V2ItemInspector.tsx | Run lifecycle rows; raw item inspector | 622 / 447 | atoms → Props | lucide | adapt | `sui/chat/rows/` |
| components/chat/ChangedFilesTree.tsx, DiffStatLabel.tsx, lib/turnDiffTree.ts | Per-turn changed files and +/− labels | 260 / 53 / 172 | contracts types → AC (`CheckoutDiff` files) | lucide | as-is (types adapted) | `sui/chat/ChangedFiles.tsx` |
| components/chat/ChatHeader.tsx | Thread header: title, branch, actions | 441 | session and threads state, commands → CQ | lucide | adapt | `sui/chat/ThreadHeader.tsx` |
| components/chat/timelineScrollAnchoring.ts, pageScrollController.ts, timelineMinimap*.ts | Scroll anchoring, stick to bottom, minimap | 182 / 307 / 95 | none | — | as-is | `sui/chat/scroll/` |
| components/chat/ThreadFindBar.tsx, threadFindHighlights.ts; useThreadFind.ts | Find in thread | 129 / 269; 386 | the hook uses query state → AC state | lucide | bar and highlights as-is; hook adapt | `sui/chat/find/` |
| components/chat/{MessageCopyButton, AgentElapsed, ExpandedImageDialog, ZoomableImage, TimelineSystemDivider, ThreadErrorBanner}.tsx | Small presentational pieces | 67 / 76 / 294 / 247 / 67 / 88 | AgentElapsed uses client-runtime DateTime → plain | lucide | as-is (AgentElapsed: adapt) | `sui/chat/` |
| pkg client-runtime/work-log/{commandLabel, presentation, itemDetail, toolPresentation, userInput, scrollAnchor}.ts | How tool calls and commands are presented | 2 730 | Effect-free except `itemDetail` (`effect/DateTime` → plain); contracts → AC `ToolCall` | — | adapt (high value: maps each `ToolCall` kind to a label, icon and detail) | `sui/chat/tools/` |
| pkg shared/{markdownPipeline, markdownLinks, codexMarkdownDirectives, toolActivity, toolOutput, threadFindText}.ts | Markdown pipeline, tool classification | 2 117 | Effect-free except `toolOutput` (`Predicate` → plain); contracts types → AC | unified, remark-gfm, remark-breaks, remark-parse, remark-rehype, rehype-sanitize (MIT ✓); **rehype-raw is removed** (D919) | adapt | `sui/markdown/pipeline/` |

## (c) Composer → SD1 Task 19

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/chat/ChatComposer.tsx | The whole composer: editor, chips, attachments, slash menu, pickers, send, queue, steer | 7 813 | about 15 state and atom modules; client-runtime dispatch and attachments; zustand → CQ (`QueueCommand`, `QueueMessage`, queue methods, `UploadChunk`/`UploadCommit`), Props | lucide | rewrite | `ps/thread/Composer.tsx` |
| ComposerPromptEditorTiptap.tsx | Tiptap editor: inline chips (files, skills, citations), mentions, paste | 1 910 | contracts types; shared clipboard and skills helpers → AC (`ListSkills`, `ListCommands`, `SearchWorkspaceFiles`) | @tiptap/core, pm, react, starter-kit (MIT ✓; **no Tiptap Pro**) | adapt (key reuse) | `sui/composer/Editor.tsx` |
| composer-rich-text-doc.ts | Markdown ↔ ProseMirror doc model, cursor coordinates | 1 573 | none | @tiptap/* extensions: blockquote, code, code-block, heading, horizontal-rule, list, task-item, task-list (MIT ✓) | as-is | `sui/composer/doc.ts` |
| composer-rich-text.ts, composer-code-block.ts, composer-list-continuation.ts, composer-undo-grouping.ts | Editor helpers | 131 / 254 / 147 / 64 | none | @tiptap/pm (MIT ✓) | as-is | `sui/composer/` |
| composer-logic.ts, composer-editor-mentions.ts | Trigger detection, mention model | 364 / 180 | contracts and shared types → AC | — | adapt | `sui/composer/` |
| pkg shared/{composerTrigger, composerInlineTokens, composerContextReferences, inlineSkills, searchRanking}.ts | `@` and `/` triggers, inline tokens, fuzzy ranking | 859 | Effect-free (types → AC) | — | as-is | `sui/composer/shared/` |
| composerDraftStore.ts | Per-thread drafts, attachments, model choice, persistence | 4 613 | `effect` Effect, Equal, Schema, Types; zustand → zustand + zod (Q819) | zustand (MIT ✓) | adapt (large; cut to what Studio uses: draft text, chips, attachments, harness and model choice) | `sui/composer/draftStore.ts` |
| promptStashStore.ts, components/chat/ComposerStashMenu.tsx | Prompt stash | 302 / 227 | `effect/Schema`; zustand → zod | zustand | adapt | `sui/composer/stash/` |
| components/chat/ComposerCommandMenu.tsx, composerSlashCommandSearch.ts, composerPromptHistory.ts | Slash and mention menu, search, ↑ history | 274 / 114 / 215 | client-runtime skill types → AC (`ListCommands`, `ListSkills`) | lucide | adapt | `sui/composer/menus/` |
| components/chat/composerAttachmentFiles.ts | File intake (drop, paste) | 177 | none | — | as-is | `sui/composer/attachments/` |
| lib/imageCompression.ts | Image downscaling and HEIC conversion | 504 | none | **heic-to (LGPL-3.0 ✗)** | adapt **without the HEIC path** | `sui/composer/attachments/compress.ts` |
| lib/attachmentUploadQueue.ts | Upload queue | 675 | `effect` Option, reactivity; atom registry; assets state → CQ (`UploadChunk`, `UploadCommit`) | — | rewrite | `ps/thread/uploads.ts` |
| components/chat/ModelPickerContent.tsx, ProviderModelPicker.tsx, ModelListRow.tsx, ModelPickerSidebar.tsx, modelPickerSearch.ts | Model picker | 1 109 / 326 / 153 / 254 / 87 | atoms, server state → CQ (`ListHarnesses`, `ListModels`) | @legendapp/list (→ VL) | adapt (pick a harness first, then a model) | `sui/composer/model-picker/` |
| components/chat/TraitsPicker.tsx, modelSelection.ts | Reasoning and speed choices | 698 / 396 | `effect` Array, Result; draft store → plain | lucide | adapt (→ `reasoning`, `modelOptions`) | `sui/composer/model-picker/` |
| components/chat/ComposerPrimaryActions.tsx, ComposerSurface.tsx, ComposerControl.tsx, composerFooterLayout.ts | Send, stop and queue buttons; layout | 399 / 101 / 127 / 241 | client-runtime dispatch (Effect-free) → Props | lucide | as-is / adapt | `sui/composer/` |
| components/chat/ComposerBanner.tsx, ComposerBannerStack.tsx | Banner slots above the composer | 369 / 377 | none | @base-ui/react (→ UI) | as-is (primitives → UI) | `sui/composer/banners/` |
| components/chat/ContextWindowMeter.tsx (+ logic 149) | Context window meter | 180 | none → AC `contextUsage` | — | as-is | `sui/composer/ContextMeter.tsx` |
| components/chat/QueuedRunsControl.tsx | Edit queued messages | 496 | threads state, commands → CQ (`WatchQueue`, queue methods) | lucide | adapt | `sui/composer/Queue.tsx` |
| components/composerContextPresentation.tsx, contextChipParts.tsx, ContextChip.tsx | Context chips: files, terminal selections, review comments | 461 / 304 / 133 | draft store | lucide | adapt | `sui/composer/chips/` |

## (d) Diff panel and review comments → SD1 Task 20

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/DiffPanel.tsx | Diff panel for a turn, the whole thread or the working tree, with a file tree, comments and find | 1 254 | atoms; `effect` DateTime, Schema; review, vcs, server and filesystem state; diff store → CQ (`WatchCheckoutDiffs`, `GetCheckoutDiff`, `GetCheckoutFileDiffText`, `WatchReviewComments`) | @pierre/diffs (Apache-2.0 ✓; the **patched** API's find calls are dropped, §53 §9.1), lucide | adapt | `sui/diff/DiffPanel.tsx` |
| components/DiffPanelShell.tsx, DiffWorkerPoolProvider.tsx | Panel chrome; the Pierre worker pool (Shiki in workers) | 116 / 167 | light `effect/Schema` → zod | @pierre/diffs | as-is (Schema stripped) | `sui/diff/` |
| components/diffs/StyledDiffCodeView.tsx, AnnotatableCodeView.tsx, DiffCommentAnnotation.tsx | Code views with inline review comments | 261 / 306 / 137 | AnnotatableCodeView sends comments to the composer through the draft store → CQ (`UpsertReviewComment`, `SendReviewToThread`) | @pierre/diffs, lucide | adapt | `sui/diff/review/` |
| components/diffs/DiffFileTree.tsx, diffFileTree.logic.ts, DiffFileStatus.tsx, useCodeViewFileReveal.ts | The diff's file tree | 236 / 132 / 39 / 33 | none | @pierre/trees (Apache-2.0 ✓, beta), @pierre/diffs | as-is | `sui/diff/tree/` |
| components/diffs/useReviewFilePatches.ts | Loads patches per file | 245 | atoms, `AsyncResult`, review state → CQ | @pierre/diffs | rewrite | `sui/diff/usePatches.ts` |
| lib/diffRendering.ts, lib/diffFileContents.ts, lib/diffCollapse.ts | Theme resolution; file contents for context expansion | 378 / 124 / 13 | `diffFileContents` uses the client-runtime → CQ (`GetCheckoutFileDiffText`) | @pierre/diffs | `diffRendering` as-is (themes → tokens); the others adapt | `sui/diff/` |
| diffPanelStore.ts, reviewCommentContext.ts, diffFileActions.ts | Panel UI store; review comment → composer context | 140 / 477 / 100 | zustand; `effect/Schema` → zod | zustand, @pierre/diffs | adapt | `sui/diff/` |
| pkg shared/gitPatchPath.ts | Patch path parsing | 140 | none | — | as-is | `sui/diff/patchPath.ts` |
| index.css lines 1711–2121 | `--diffs-*` theming | about 410 | — | — | adapt (mapped onto `@loams/ui` tokens; no raw colours) | `sui/diff/diffs.css` |

## (e) File tree and file viewer → SD1 Task 21

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/files/FileBrowserPanel.tsx | Project file tree panel | 574 | `localApi`, query state → CQ (`ListWorkspaceDirectory`, `WatchWorkspaceFiles`) | @pierre/trees (Apache-2.0 ✓, beta), lucide | adapt | `sui/files/FileBrowser.tsx` |
| components/files/FilePreviewPanel.tsx | Viewer: code, markdown, media, CSV | 1 507 | `effect/Schema`; assets, filesystem, preview and session state; commands → CQ (`ReadWorkspaceFile`, `ReadWorkspaceImage`) | @pierre/diffs | adapt **read-only** (Studio has no editor, §53 §2.2) | `sui/files/FileViewer.tsx` |
| components/files/{FileBreadcrumbs, ReadOnlySourcePreview, DelimitedTablePreview, FileMarkdownPreview, fileSurfaceChrome}.tsx | Viewer pieces | 295 / 51 / 63 / 34 / 163 | none | lucide | as-is | `sui/files/` |
| components/files/ProjectFilePicker.tsx | Quick-open file picker | 163 | atoms → CQ (`SearchWorkspaceFiles`) | — | adapt | `sui/files/FilePicker.tsx` |
| components/files/projectFilesQueryState.ts, useDirectoryEntries.ts | Directory listing queries | 289 / 147 | atoms; `effect` Cause, Option, Schema, reactivity → CQ | — | rewrite | `sui/files/useDirectory.ts` |
| components/files/{fileTreeExpansion, fileTreePathReconciliation, fileTreeDragMention, filePath, filePreviewMode}.ts | Tree helpers | 55 / 32 / 95 / 60 / 76 | none | — | as-is | `sui/files/` |
| components/files/fileSaveCoordinator.ts | Saves from the editor | 85 | — | — | skip (read-only) | — |
| pierre-icons.ts, pierre-tree-theme.ts, components/chat/PierreEntryIcon.tsx | File icons and tree theme | 197 / 27 / 96 | shared file-preview helpers | @pierre/trees | as-is (theme → tokens) | `sui/files/icons/` |
| components/search/ProjectContentSearchDialog.tsx, HighlightedSearchLine.tsx | Content search (ripgrep style) | 337 / 183 | query state, right-panel store → CQ (`SearchWorkspaceFiles`; content search is the new `SearchWorkspaceContent`, G23, Task 11) | @pierre/diffs | adapt | `sui/files/search/` |
| *(fallback)* | A tree on TanStack Virtual if `@pierre/trees` (beta) fails review | — | — | @tanstack/react-virtual (MIT ✓) | ours | `sui/files/VirtualTree.tsx` |

## (f) Terminal → SD1 Task 22

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| terminal/ghostty/{core, surface, renderer, runtime, keyCodes}.ts + `vendor/ghostty-vt.wasm` (630 932 B) + Nerd Font | libghostty-vt wasm VT core and a Canvas2D renderer | 4 170 | none | libghostty-vt (MIT, "Copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors"); Symbols Nerd Font (MIT) | **skip in SD1**: xterm.js instead (§53 D920, Q818). A prebuilt wasm needs a reproducible build first | — |
| components/ThreadTerminalDrawer.tsx | Terminal drawer: tabs, split, links, confirm on close | 1 902 | atoms, `effect/Schema`; terminal, session, server and preview state; commands → CQ (`OpenTerminal`, `SubscribeTerminal`, `WriteTerminal`, `ResizeTerminal`, `CloseTerminal`) | lucide; **@xterm/xterm, @xterm/addon-fit (MIT ✓)** replace ghostty | adapt | `sui/terminal/TerminalDrawer.tsx` |
| terminalUiStateStore.ts | Terminal tab and split state | 781 | zustand | zustand | as-is | `sui/terminal/uiStore.ts` |
| terminal-links.ts, lib/terminalContext.ts, lib/terminalClose*.ts | Link detection; selection → composer context; close rules | 161 / 102 / 78 | none | — | as-is | `sui/terminal/` |
| state/terminalSessions.ts + pkg client-runtime/state/{terminalSession, terminalOutput}.ts | Attach and replay buffering | 200 + 533 | Effect-free → AC `TerminalEvent` (`seq`, base64) | — | adapt | `sui/terminal/session.ts` |

## (g) Branch and git toolbar → SD1 Task 23

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/BranchToolbar.tsx, BranchToolbar.logic.ts | Branch and worktree toolbar | 865 / 329 | draft store, `localApi`, entities; logic uses `effect/Schema` and shared git → CQ (`ListRefs`, `ListWorktrees`), zod | lucide | adapt | `sui/git/BranchToolbar.tsx` |
| components/BranchToolbarBranchSelector.tsx, BranchPicker.tsx, WorktreeBaseBranchPicker.tsx | Branch pickers with paged refs | 827 / 307 / 126 | vcs, query and branch state; commands → CQ (`ListBranches`, `ListRefs`, `SwitchRef`) | @legendapp/list (→ VL) | adapt | `sui/git/pickers/` |
| components/BranchToolbarEnvModeSelector.tsx, PreviousWorktreeItemContent.tsx | Local or new worktree; reuse a previous one | 177 / 19 | light | lucide | adapt | `sui/git/` |
| components/BranchToolbarEnvironmentSelector.tsx | T3 environment (host) choice | 144 | — | — | skip (Studio's host selector is the shell's) | — |
| components/GitActionsControl.tsx + .logic.ts | Commit, push, create PR, pull as one "stacked action" | 2 159 / 488 | atoms, `effect/Option`; vcs, source-control and threads state; logic Effect-free → CQ (`CommitWorktree`, `PushBranch`, `CreateChangeRequest`, `FetchAll`) | @base-ui/react (→ UI) | logic adapt; component adapt | `sui/git/GitActions.tsx` |
| pkg client-runtime/state/gitActions.ts | Git action progress state machine | 354 | Effect-free | — | adapt | `sui/git/actions.ts` |
| state/sourceControlActions.ts, client-runtime/state/vcsAction.ts | The rest of the action plumbing | 413 + 600 | Effect-heavy | — | rewrite on CQ mutations | `ps/git/` |
| components/chat/WorktreeSetupCard.tsx | Worktree creation and setup progress | 436 | contracts types → AC (`CreateWorktree` result, setup action) | lucide | as-is (types adapted) | `sui/git/WorktreeSetupCard.tsx` |
| components/ProjectScriptsControl.tsx, projectScripts.ts, components/projectScriptEditor.tsx | Project script buttons and editor | 482 / 112 / 526 | atoms, `effect/Schema` → CQ (`ListProjectActions`, `UpsertProjectAction`, `RunProjectAction`) | lucide | adapt | `sui/git/ProjectActions.tsx` |
| components/PullRequestThreadDialog.tsx | "Open PR as a thread" | 334 | vcs, query and session state | @tanstack/react-pacer | adapt later (after SD1) | — |
| components/pullRequest/** (69 files) | The full PR review product | 19 086 | atoms, `effect`, zustand, forge RPCs | — | **skip** (Forgejo's own UI and SF1's panels cover it) | — |
| pkg shared/{git, sourceControl, changeRequestUrl}.ts | Git status and branch helpers; PR URL parsing | 993 | `git.ts` uses `effect` Array, Result → plain | — | adapt | `sui/git/shared/` |

## (h) Command palette and keybindings → SD1 Task 24

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/CommandPalette.tsx | Palette: threads, projects, files, actions, themes | 3 660 | atoms, `effect/Option`, about 20 state modules → the shell's `palette.command` slot + CQ | @legendapp/list (→ VL), → HR | rewrite | `ps/palette/StudioPalette.tsx` |
| components/CommandPalette.logic.ts | Builds and ranks items | 623 | `effect` Array, Result → plain | — | adapt | `sui/palette/logic.ts` |
| components/CommandPaletteResults.tsx, CommandPaletteContent.tsx, commandPaletteBus.ts | Result list; open and close | 247 / 100 / 39 | none | @legendapp/list (→ VL), lucide | adapt (VL) | `sui/palette/` |
| components/ui/command.tsx, ui/autocomplete.tsx | Palette primitive on Base UI Dialog and Autocomplete (no cmdk) | 261 / 257 | none | @base-ui/react (MIT ✓) | adapt **into `@loams/ui`** if it lacks a command primitive (ruling 2) | `ui/command.tsx` |
| keybindings.ts + pkg shared/keybindings.ts | Keybinding resolution, `when` expressions, shortcut display | 458 + 469 | Effect-free (contracts command literals → our own list) | — | adapt (high value) | `sui/keybindings/` |
| shortcutModifierState.ts | Modifier-key overlay | 123 | none | — | as-is | `sui/keybindings/` |
| components/settings/KeybindingsSettings.tsx + .logic.ts | Keybinding editor | 1 539 / 441 | server state, commands; logic Effect-free → client store (§53 §5.10) | lucide | logic as-is; UI adapt | `ps/settings/Keybindings.tsx` |

## (i) Approvals and questions → SD1 Task 18 (inline) and Task 13

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/chat/ComposerPendingApprovalPanel.tsx | Approval request panel above the composer | 68 | none | — | as-is | `sui/approvals/PendingApproval.tsx` |
| components/chat/ComposerPendingApprovalActions.tsx | Approve, deny and always buttons from the provider's options | 116 | contracts types → AC `Approval` (G17) and `ApprovalDecision` (`once`, `always`, `deny`) | lucide | adapt | `sui/approvals/ApprovalActions.tsx` |
| components/chat/ComposerPendingUserInputPanel.tsx, pendingUserInput.ts, questionAttachments.ts | Agent questions: choices, free text, attachments | 308 / 223 / 63 | contracts types; zustand → AC `inputRequested`, CQ (`AnswerQuestion` → `QueueCommand respondInput`) | lucide | adapt (reconciled with DD1's `QuestionCard`, Q802) | `sui/approvals/Question.tsx` |
| components/chat/SecretRequestCard.tsx | Asks for a credential | 179 | server state, commands | — | skip (secrets never pass through the renderer, §50 §12) | — |
| pkg client-runtime/state/threadRequests.ts | Pending request derivation | 173 | `effect/DateTime` → plain | — | adapt | `sui/approvals/requests.ts` |
| components/permissions/* | macOS permission checklist (not agent approvals) | 102 | — | — | skip | — |

## (j) Plan, tasks and settings → SD1 Tasks 18 and 28

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/chat/ProposedPlanCard.tsx, proposedPlan.ts, ComposerPlanFollowUpBanner.tsx | Plan-mode proposed plan; "implement" follow-up | 287 / 106 / 20 | projects and session state, commands → AC plan parts, CQ | lucide | adapt | `sui/chat/plan/` |
| components/chat/ComposerTasksBadge.tsx | Todo list progress badge | 235 | none | lucide | as-is | `sui/chat/plan/TasksBadge.tsx` |
| components/settings/settingsLayout.tsx | Settings shell | 594 | `effect/Equal`, atoms | — | skip (the console's `console.settings.section` is the shell) | — |
| components/settings/SettingsPanels.tsx and the other panels (connections, providers, scheduled tasks, diagnostics, storage, source control) | T3's settings | about 44 000 | heavy | — | skip | — |
| components/settings/ThemeSettings.tsx, ThemeEditorPanel.tsx, themePalette.ts, hooks/useTheme.ts | Theme picker and VS Code theme import | 1 045 / 1 313 / 1 662 / 679 | `effect/Schema`, Equal; zustand | culori (MIT ✓) | skip (Loams has one token set, ruling 1) | — |
| hooks/useSettings.ts | Client and server settings atoms | 816 | atoms | — | skip | — |

## (k) UI primitives and generic hooks → SD1 Tasks 1 and 16

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| ui/{button, badge, input, textarea, label, kbd, separator, skeleton, spinner, switch, checkbox, radio-group, toggle, toggle-group, group, input-group, number-field, empty, alert, table, collapsible, collapsible-section-header, middle-truncate, refresh-icon, panel-tab-close-button, discovery-list, draft-input, standalone-page, wizard} | Base UI and cva primitives | about 2 000 | none | @base-ui/react (MIT ✓), class-variance-authority (Apache-2.0 ✓), lucide (ISC ✓) | adapt **into `@loams/ui`**, only where `@loams/ui` lacks the primitive; re-styled to tokens, `rounded-none`/`rounded-pill` | `ui/*` |
| ui/{dialog, dialog-styles, alert-dialog, sheet, popover, tooltip, menu, select, combobox, autocomplete, scroll-area, preview-card} | Overlay and listbox primitives | 2 374 | none | @base-ui/react, @legendapp/list (→ VL) | adapt into `@loams/ui`'s `overlay.tsx` (merge with the `console/ui-stack` work, never a second copy) | `ui/overlay.tsx` and siblings |
| ui/sidebar.tsx (+ sidebarState) | Resizable, collapsible sidebar | 859 | `effect/Schema` (stored width) → zod | — | adapt | `ui/sidebar.tsx` |
| ui/toast.tsx, toast.logic.ts, toastHelpers.ts, anchoredCopyToast.ts | Toasts scoped to threads | 800 / 131 / 66 / 33 | draft store, router params, contracts → Props, HR | morphicons (MIT ✓) | adapt only the thread-scoping logic onto the shell's toasts; skip morphicons | `sui/toast/` |
| ui/calendar.tsx, color-picker.tsx, qr-code.tsx | Calendar, colour picker, QR | 112 / 225 / 81 | — | @daypicker/react (MIT ✓) | skip | — |
| lib/utils.ts | `cn` and more | 50 | draft store, keybindings, `effect` Hex | tailwind-merge (MIT ✓) | take `cn` only (`@loams/ui` may already have it, `cx.ts`) | `ui/cx.ts` |
| index.css (tokens, lines 89–260) | Tailwind 4 theme | about 170 | — | — | skip (Loams' `tokens.css` is the only token set); T3's semantic names (`diff-addition`, `warning`) are mapped to ours | — |
| hooks/{useMediaQuery, useLocalStorage, useResizableWidth, useResizeDrag, useCopyToClipboard}, lib/{lruCache, observeResize}, components/AnimatedHeight, timestampFormat.ts | Generic hooks and utilities | about 1 400 | some `effect/Schema` storage decoding → zod, wrapped in try/catch | — | adapt | `sui/hooks/`, `sui/lib/` |

## (l) Parallel agents, lineage and worktrees → SD1 Task 25

T3 Code **has no parallel-run compare view**. Its nearest features are fork, merge-back, delegated tasks and subagent workflows. Studio's run groups (§53 §5.5) are new code.

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/chat/ThreadRelationshipsControl.tsx | Parent, child, fork and subagent popover | 702 | atoms; `effect` Array, DateTime; client-runtime relationships → AC (`parentChatId`), CQ | lucide | adapt (also shows run-group siblings) | `sui/groups/Relationships.tsx` |
| components/chat/ThreadLineageWorkflowRow.tsx, ProviderSubagentBar.tsx, SubagentTooltipContent.tsx, agentSpawnSummary.ts | Workflow rows, subagent bar | 266 / 93 / 191 / 64 | client-runtime subagent runtime (`effect` Array, DateTime) → AC `subagent` events | — | adapt | `sui/chat/subagents/` |
| pkg client-runtime/state/{threadRelationships, threadWorkflows, subagentDisplay, subagentRuntime, threadSubagents}.ts | Lineage and subagent derivations | 989 | light `effect` → plain | — | adapt | `sui/chat/subagents/rules/` |
| *(new)* Run-group grid, diff matrix, side-by-side compare, resolve dialog | §53 §5.5 | — | CQ (G12–G16) | @pierre/diffs | ours | `sui/groups/`, `ps/groups/` |

## (m) Shared utilities (`packages/shared/src`) → used across tasks

| Upstream path | Lines | Deps → replacement | Verdict |
|---|---|---|---|
| `path.ts`, `fileLinks.ts`, `filePreview.ts` | 123 / 183 / 219 | none | as-is |
| `markdownGithubAlerts.ts`, `markdownListIndentation.ts` | 72 / 143 | none | as-is |
| `terminalLabels.ts`, `orchestrationTiming.ts` | 49 / 88 | types → AC | adapt |
| `semver.ts`, `delimitedPreview.ts`, `projectScripts.ts` | 234 / 56 / 88 | none | as-is |
| `htmlRender.ts` | 417 | renders HTML strings | **skip** (D919) |
| `usageFormat.ts` | 263 | none | adapt (token counts for G20; no prices, CONTRIBUTING's open-core rule) |
| `themePalettes.ts` | 954 | — | skip |
| `packages/contracts/**` (35 668, all Effect Schema) | — | — | **skip as code**; read as the schema reference for `loams.agentd.v1` (`orchestrationV2.ts`, `git.ts`, `vcs.ts`, `terminal.ts`, `keybindings.ts`, `checkpointDiff.ts`, `worktreeSetup.ts`) |
| `packages/client-runtime/rpc/**`, `connection/**`, `environments/**`, `state/*.ts` atom bindings, `rpc/atomRegistry.ts` | — | Effect RPC over WebSocket | **skip** (replaced by `loams.agentd.v1` and connect-query) |

## (n) Preview → SD1 Task 26

| Upstream path | Purpose | Lines | Deps → replacement | Third-party (licence) | Verdict | Target |
|---|---|---|---|---|---|---|
| components/preview/{PreviewChromeRow, PreviewEmptyState, PreviewUnreachable, PreviewMoreMenu}.tsx | URL bar and empty or unreachable states | 320 / 96 / 144 / 207 | preview and device state → CQ (`WatchPreviews`) | lucide | adapt | `sui/preview/` |
| components/preview/PreviewView.tsx, PreviewPanel.tsx, PreviewPanelShell.tsx | The preview surface | 1 154 / 63 / 162 | atoms, `effect`, many stores | — | rewrite (desktop: a `WebContentsView` placed by main, as D678; web: the remote `/preview/` URL, Q806) | `ps/preview/` |
| components/preview/useDiscoveredLocalServers.ts, openDiscoveredPort.ts | Discovered dev servers | 105 / 33 | server stream → CQ (`WatchPreviews`) | — | adapt | `sui/preview/` |
| browser/** (30 files) | Electron webview host, server-run browser streaming, recording | 5 360 | Effect-heavy | — | skip | — |

## Skipped wholesale

| Area | Why |
|---|---|
| `cloud/`, `components/{auth, clerk, cloud, onboarding, usage, device}`, `onboarding/` | Cloud, Clerk auth, marketing onboarding, billing and usage, mobile device mirroring (owner direction) |
| `apps/server/**` | T3's server. `loams-agentd` replaces it (owner direction) |
| `apps/desktop/**` | T3's Electron shell. `apps/desktop-electron` is ours |
| `apps/mobile/**`, `apps/marketing/**` | Out of scope |
| `packages/provider-*`, `effect-acp`, `effect-codex-app-server` | Provider-specific code; the daemon's harness drivers own providers |
| `packages/source-control-*` | Forge integrations; the daemon gets Forgejo and GitHub through G4 and G11 |
| `packages/ssh`, `packages/tailscale` | Remote environments; Studio's remote is a remote agentd (§53 §6.3) |
| `components/pullRequest/**`, `routes/_chat.pull-requests.tsx` | A whole PR review product; Forgejo and SF1 cover it |
| `components/settings/**` (except the keybinding logic) | T3's settings model |
| `patches/*` | Not vendored. The `@pierre/diffs` patch's find-in-diff is reconsidered in Task 20 |

## Third-party dependencies added to the pnpm catalog

| Package | Version T3 uses | Licence (npm, 2026-10-13) | Allowed? | Used by |
|---|---|---|---|---|
| `@pierre/diffs` | 1.5.2 | Apache-2.0 | ✓ | diff panel, file viewer, highlighting |
| `@pierre/trees` | 1.0.0-beta.6 | Apache-2.0 | ✓ (beta; fallback kept) | file tree, diff tree |
| `shiki` | 4.2.0 (through `@pierre/diffs`) | MIT | ✓ | highlighting |
| `@tiptap/core`, `pm`, `react`, `starter-kit` and the listed extensions | 3.31.3 | MIT | ✓ (no Pro) | composer |
| `@dnd-kit/core`, `sortable`, `modifiers`, `utilities` | 6.3.1 / 10.0.0 / 9.0.0 / 3.2.2 | MIT | ✓ | sidebar reorder |
| `zustand` | 5.0.14 | MIT | ✓ (Q819) | client UI stores |
| `react-markdown`, `remark-gfm`, `remark-breaks`, `remark-parse`, `remark-rehype`, `rehype-sanitize`, `unified`, `hast-util-to-html`, `hast-util-to-jsx-runtime` | 10.1.0 / 4.0.1 / … | MIT | ✓ | markdown (not `rehype-raw`) |
| `@xterm/xterm`, `@xterm/addon-fit` | (DD1's pin) | MIT | ✓ | terminal |
| `diff` | 9.0.0 (through `@pierre/diffs`) | BSD-3-Clause | ✓ | — |
| `preact` | 11.0.0-beta.0 (through `@pierre/trees`) | MIT | ✓ (beta) | — |
| `@legendapp/list` | 3.3.5 (patched) | MIT | ✓ but **not added**: VL replaces it (ruling 5) | — |
| `heic-to` | 1.5.x | **LGPL-3.0** | **✗** | not added; the HEIC path is dropped |
| `jszip` | 3.10.1 | MIT OR GPL-3.0-or-later | ✓ under MIT, but not needed | not added |
| `mermaid`, `dompurify` | 11.17.2 / 3.4.16 | MIT / MPL-2.0 OR Apache-2.0 | ✓ | `dompurify` is already the markdown fence; `mermaid` is not added in SD1 |
| `morphicons`, `@daypicker/react`, `culori`, `@formkit/auto-animate` | — | MIT | ✓ | not added |
| `effect`, `@effect/atom-react`, `@tanstack/react-router` | — | MIT | — | **not added** (no Effect; the shell's router) |
