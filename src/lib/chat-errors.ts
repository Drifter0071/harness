// ChatError → UI actions (spec §7.5 + §9).
//
// Every `chat-error` event maps to:
//   - toast: text for the toast host (empty = silent, used by `cancelled`)
//   - bubble: replacement text for the optimistic assistant message
//     (null = remove the message, undefined = keep the partial content
//     streamed so far — used by `cancelled`, which must NOT discard the
//     already-generated segment)
//
// Pure function so the mapping is unit-testable without the UI.

import type { ChatError } from "./ipc";

export interface ChatErrorActions {
  toast: string;
  /** null → remove the optimistic message entirely; undefined → keep
   *  whatever partial content already streamed. */
  bubble: string | null | undefined;
  /** Re-enable the composer (false for `cancelled`/`auth`). */
  reenableComposer: boolean;
}

function truncated(text: string, max = 200): string {
  return text.length > max ? `${text.slice(0, max)}...` : text;
}

export function chatErrorActions(err: ChatError): ChatErrorActions {
  switch (err.kind) {
    case "cancelled":
      // Keep the partial response in the chat; only the UI decides to
      // drop the bubble when nothing was streamed at all.
      return { toast: "", bubble: undefined, reenableComposer: true };
    case "auth":
      return {
        toast: "Invalid API key — re-add in Settings.",
        bubble: "(auth failed: re-add API key)",
        reenableComposer: false,
      };
    case "rate_limit": {
      const secs = err.retryAfterSeconds;
      return {
        toast: secs
          ? `Rate limited — try again in ${secs}s`
          : "Rate limited — try again shortly.",
        bubble: secs
          ? `(rate limited — try again in ${secs}s)`
          : "(rate limited)",
        reenableComposer: true,
      };
    }
    case "network":
      return {
        toast: "Network error — please retry.",
        bubble: truncated(err.error),
        reenableComposer: true,
      };
    case "provider":
      return {
        toast: "Provider error — please retry.",
        bubble: "(provider error)",
        reenableComposer: true,
      };
    case "bad_request":
      return {
        toast: truncated(err.error),
        bubble: truncated(err.error),
        reenableComposer: true,
      };
    case "internal":
      return {
        toast: "Internal error — please retry.",
        bubble: "(internal error)",
        reenableComposer: true,
      };
  }
}
