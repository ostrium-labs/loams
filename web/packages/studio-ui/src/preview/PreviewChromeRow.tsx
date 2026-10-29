// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/preview/PreviewChromeRow.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// @loams/ui's Button, Tooltip and InputGroup on tokens; the capture, picture-in-picture and annotation controls are dropped (T3's browser streaming and recording are not carried, inventory (n)); "Open in browser" is always shown when given. Loams' changes are Apache-2.0.
import { Button, InputGroup, InputGroupAddon, InputGroupInput, Tooltip } from "@loams/ui";
import { ArrowLeft, ArrowRight, ExternalLink, RotateCw, X } from "lucide-react";
import {
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react";

interface Props {
  url: string;
  loading: boolean;
  canGoBack: boolean;
  canGoForward: boolean;
  refreshDisabled: boolean;
  inputDisabled?: boolean | undefined;
  /** Bumping this value re-focuses and selects the URL input. */
  focusUrlNonce?: number | undefined;
  onBack: () => void;
  onForward: () => void;
  onRefresh: () => void;
  onSubmit: (url: string) => void;
  /** When provided, renders an "Open in browser" affordance to the right. */
  onOpenInBrowser?: (() => void) | undefined;
  /** When provided, renders a close button that returns to the server list. */
  onClose?: (() => void) | undefined;
  /** Trailing slot rendered after the URL input. */
  trailingActions?: ReactNode;
}

const NOOP = () => {};

export function PreviewChromeRow({
  url,
  loading,
  canGoBack,
  canGoForward,
  refreshDisabled,
  inputDisabled,
  focusUrlNonce,
  onBack,
  onForward,
  onRefresh,
  onSubmit,
  onOpenInBrowser,
  onClose,
  trailingActions,
}: Props) {
  // @loams/ui's InputGroupInput takes no ref; the group does, and holds one input.
  const groupRef = useRef<HTMLDivElement | null>(null);
  const inputRef = {
    get current(): HTMLInputElement | null {
      return groupRef.current?.querySelector("input") ?? null;
    },
  };
  const [draft, setDraft] = useState(url);
  const [inputFocused, setInputFocused] = useState(false);

  // oxlint-disable-next-line react/exhaustive-deps -- inputRef reads the group's ref.
  useEffect(() => {
    if (focusUrlNonce == null) return;
    inputRef.current?.focus();
  }, [focusUrlNonce]);

  const submit = (event?: FormEvent | KeyboardEvent) => {
    event?.preventDefault();
    const next = draft.trim();
    if (next.length === 0) return;
    onSubmit(next);
    inputRef.current?.blur();
  };

  return (
    <div className="relative">
      <form
        onSubmit={submit}
        className="flex h-10 min-h-10 shrink-0 items-center gap-1 border-b border-rule-soft bg-bg px-2"
        data-preview-chrome
      >
        <fieldset className="m-0 flex items-center gap-0.5 border-0 p-0" aria-label="Navigation">
          <Tooltip content="Back">
            <Button
              variant="quiet"
              size="icon"
              onClick={canGoBack ? onBack : NOOP}
              disabled={!canGoBack}
              aria-label="Back"
            >
              <ArrowLeft aria-hidden className="size-3.5" />
            </Button>
          </Tooltip>
          <Tooltip content="Forward">
            <Button
              variant="quiet"
              size="icon"
              onClick={canGoForward ? onForward : NOOP}
              disabled={!canGoForward}
              aria-label="Forward"
            >
              <ArrowRight aria-hidden className="size-3.5" />
            </Button>
          </Tooltip>
          <Tooltip content={loading ? "Loading…" : "Reload"}>
            <Button
              variant="quiet"
              size="icon"
              onClick={refreshDisabled ? NOOP : onRefresh}
              disabled={refreshDisabled}
              aria-label="Reload"
            >
              <RotateCw aria-hidden className={`size-3.5 ${loading ? "animate-spin" : ""}`} />
            </Button>
          </Tooltip>
        </fieldset>

        <InputGroup ref={groupRef} className="h-7 flex-1">
          <InputGroupInput
            value={inputFocused ? draft : url}
            onChange={(event) => setDraft(event.target.value)}
            onFocus={() => {
              setDraft(url);
              setInputFocused(true);
              queueMicrotask(() => inputRef.current?.select());
            }}
            onBlur={() => setInputFocused(false)}
            onKeyDown={(event) => {
              if (event.key === "Enter") submit(event);
              if (event.key === "Escape") {
                event.preventDefault();
                setDraft(url);
                inputRef.current?.blur();
              }
            }}
            placeholder="localhost:5173"
            spellCheck={false}
            disabled={inputDisabled}
            aria-label="Preview address"
            className={/* mono-ok: path */ "font-mono"}
            data-preview-url-input
          />
          {onOpenInBrowser && !inputFocused ? (
            <InputGroupAddon align="end">
              <Tooltip content="Open in browser">
                <Button
                  variant="quiet"
                  size="icon"
                  onClick={onOpenInBrowser}
                  aria-label="Open in browser"
                >
                  <ExternalLink aria-hidden className="size-3.5" />
                </Button>
              </Tooltip>
            </InputGroupAddon>
          ) : null}
        </InputGroup>
        {trailingActions}
        {onClose ? (
          <Tooltip content="Close preview">
            <Button variant="quiet" size="icon" onClick={onClose} aria-label="Close preview">
              <X aria-hidden className="size-3.5" />
            </Button>
          </Tooltip>
        ) : null}
      </form>
      {loading ? (
        <div
          aria-hidden
          className="pointer-events-none absolute bottom-0 left-0 z-10 h-0.5 w-full animate-pulse bg-accent"
        />
      ) : null}
    </div>
  );
}
