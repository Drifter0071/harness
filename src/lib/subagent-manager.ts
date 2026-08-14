// Subagent event router + lifecycle.
//
// The agent loop emits the same `agent-*` event names for both the
// orchestrator turn (request id = `agent-...`) and dispatched
// subagent turns (request id = `sub-...`). This module splits that
// stream by the `sub-` prefix, leaving everything else to
// `session-manager` for the orchestrator chat.
//
// Mounted once at the App root alongside `useProjectLifecycle`.
// Also runs the `list_subagents` resync on mount so a WebView
// reload reopens every still-pending or recently-finished subagent
// window (their live content is gone, but the registry record is the
// "final answer + status" the UI needs to render the closed window).

import { useEffect } from "react";
import { useStore } from "./store";
import { agentEvents, ipc, ORCHESTRATOR_AGENT_ID, SUBAGENT_REQUEST_PREFIX } from "./ipc";
import type { SubagentRecord } from "./ipc";
import { showToast } from "./toast";
import { DEFAULT_CHAT_RATIO } from "./wm";

function isSubagentRequestId(id: string | null | undefined): boolean {
  return typeof id === "string" && id.startsWith(SUBAGENT_REQUEST_PREFIX);
}

/** The chat ratio is no longer auto-cleared on terminal events —
 *  the right region stays open as long as the subagent registry has
 *  any entries, and the chat keeps its locked width for the whole
 *  lifetime of those windows. The orchestrator's next turn-start
 *  (or a full registry wipe) is what reclaims the full chat width. */

export function useSubagentManager(): void {
  // ── Live event routing (only sub- request ids) ──
  useEffect(() => {
    const reg = (p: Promise<() => void>): Promise<() => void> => {
      p.catch(() => {});
      return p;
    };
    const unsubs: Promise<() => void>[] = [];

    unsubs.push(
      reg(
        agentEvents.onTurnStart((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          const st = useStore.getState();
          if (!e.subagent) {
            // Backend contract: sub- ids always carry subagent identity.
            return;
          }
          // Lock the chat ratio the moment the first subagent spawns
          // — the chat column width is then fixed for the rest of
          // the session.
          if (st.chatRatio === null) {
            st.setChatRatio(DEFAULT_CHAT_RATIO);
          }
          const rec: SubagentRecord = {
            requestId: e.requestId,
            personalityId: e.subagent.personalityId,
            name: e.subagent.name,
            role: e.subagent.role,
            task: e.subagent.task,
            status: "working",
            modelId: e.subagent.modelId,
            parentRequestId: ORCHESTRATOR_AGENT_ID,
            startedAt: new Date().toISOString(),
          };
          st.upsertSubagent(rec);
          st.ensureSubagentWindow(rec);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onChunk((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          useStore.getState().appendSubagentContent(e.requestId, e.delta);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onReasoning((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          useStore.getState().appendSubagentReasoning(e.requestId, e.delta);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onToolStart((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          useStore.getState().appendSubagentToolCall(e.requestId, {
            id: e.callId,
            name: e.name,
            arguments:
              e.arguments === undefined || e.arguments === null
                ? undefined
                : (() => {
                    try {
                      const s =
                        typeof e.arguments === "string"
                          ? e.arguments
                          : JSON.stringify(e.arguments);
                      return s.length > 0 ? s : undefined;
                    } catch {
                      return String(e.arguments);
                    }
                  })(),
            status: "running",
          });
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onToolResult((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          useStore.getState().updateSubagentToolCall(e.requestId, e.callId, {
            status: e.status,
            summary: e.summary,
          });
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onApprovalRequest((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          useStore.getState().setSubagentApproval(e.requestId, {
            approvalId: e.approvalId,
            name: e.name,
            summary: e.summary,
          });
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onApprovalResult((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          const st = useStore.getState();
          const w = st.subagentWindows[e.requestId];
          if (w?.approval?.approvalId === e.approvalId) {
            st.setSubagentApproval(e.requestId, null);
          }
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onDone((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          const st = useStore.getState();
          const w = st.subagentWindows[e.requestId];
          const finalContent =
            e.content && e.content.length > 0 ? e.content : w?.content ?? "";
          st.finalizeSubagentWindow(e.requestId, {
            status: "done",
            finalContent,
            content: finalContent,
            reasoning:
              e.reasoning && e.reasoning.length > 0
                ? e.reasoning
                : w?.reasoning ?? "",
          });
          // Auto-dismiss: after a short read window the window fades
          // out so the right region doesn't accumulate stale viewers
          // forever. The chat reflows back as soon as the last
          // subagent is gone.
          scheduleAutoDismiss(e.requestId);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onError((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          const st = useStore.getState();
          const w = st.subagentWindows[e.requestId];
          const status =
            e.kind === "cancelled" ? "cancelled" : "error";
          st.finalizeSubagentWindow(e.requestId, {
            status,
            error: e.message,
            content: w?.content ?? "",
            reasoning: w?.reasoning ?? "",
          });
          // Same auto-dismiss as the done path: 3s read window
          // after the turn ends, then the window is removed.
          scheduleAutoDismiss(e.requestId);
        })
      )
    );

    // Health Agent → subagent inbox delivery. The user can't message
    // subagents; only the Health Agent can. The surfaced bubble is a
    // read-only "Health Monitor" line in the subagent viewer.
    unsubs.push(
      reg(
        agentEvents.onSubagentMessage((e) => {
          if (!isSubagentRequestId(e.requestId)) return;
          useStore.getState().appendSubagentInbox(e.requestId, e.message);
        })
      )
    );

    return () => {
      unsubs.forEach((p) => p.then((u) => u()).catch(() => {}));
    };
  }, []);

  // ── Orchestrator turn-start → wipe past subagent windows ──
  // The orchestrator's turn-start event has a non-`sub-` request id,
  // so it falls through every handler in the block above untouched.
  // Listen for it here as the "new round" signal: any subagent
  // windows from the previous turn are stale context, and the new
  // round gets its own clean right-hand region. The chat ratio is
  // cleared in the same shot so the chat reflows back to full width
  // before the first new subagent lands and re-locks it.
  //
  // In normal flow the previous orchestrator turn has already
  // finished (it `await`s every `delegate_subagent` call), so all
  // subagents are terminal by the time the new turn fires. The
  // working-subagent branch is defensive: if anything slipped
  // through, we still cancel it so it doesn't leak into the new
  // round.
  useEffect(() => {
    const reg = (p: Promise<() => void>): Promise<() => void> => {
      p.catch(() => {});
      return p;
    };
    const unsub = reg(
      agentEvents.onTurnStart((e) => {
        if (isSubagentRequestId(e.requestId)) return;
        const st = useStore.getState();
        if (st.subagents.length === 0) return;
        for (const rec of st.subagents) {
          if (rec.status === "working") {
            ipc.cancelAgent(rec.requestId).catch(() => {});
          }
        }
        clearAllDismissTimers();
        st.clearAllSubagents();
        st.clearChatRatio();
      })
    );
    return () => {
      unsub.then((u) => u()).catch(() => {});
    };
  }, []);

  // ── Registry resync on WebView mount ──
  // After a reload, the live event stream starts blank. Pull the
  // current snapshot from the backend and rebuild the mirror + open
  // a window for every still-tracked subagent. Live content for
  // in-flight subagents is gone; the registry record carries the
  // final content + status of finished ones, and the orchestrator
  // turn can still be re-observed (the user just sees the eventual
  // terminal content if the agent loop reports a fresh done for a
  // request id the frontend never saw stream).
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await ipc.listSubagents();
        if (cancelled) return;
        const st = useStore.getState();
        // Lock the chat ratio whenever the registry has any entries
        // (working OR finished — finished windows still occupy the
        // right region so the user can read their final answer). The
        // ratio is cleared only by an empty registry or by the next
        // orchestrator turn-start.
        if (list.length > 0 && st.chatRatio === null) {
          st.setChatRatio(DEFAULT_CHAT_RATIO);
        }
        st.setSubagents(list);
        for (const rec of list) {
          st.ensureSubagentWindow(rec);
          // For terminal records (Done/Error/Cancelled) the registry
          // is the source of truth — fold the final content into the
          // window state so the renderer shows something on reload.
          if (rec.status !== "working" && rec.finalContent !== undefined) {
            st.finalizeSubagentWindow(rec.requestId, {
              status: rec.status,
              finalContent: rec.finalContent,
              content: rec.finalContent,
              reasoning: "",
            });
          }
        }
      } catch (err) {
        // Best-effort: a stale list is harmless. The live stream
        // (when the orchestrator's next turn runs) still rebuilds
        // the windows from the turn-start event.
        showToast(`Could not load subagent list: ${err}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);
}

// ── Auto-dismiss after terminal ──
// A finished subagent window lives for SUBAGENT_DISMISS_MS so the
// user has a beat to read the final answer, then is removed. The
// timer is keyed by request id; if a new orchestrator turn starts
// before it fires, `clearAllSubagents` runs and the registry is
// empty by the time the callback would have run — we still bail
// defensively (the subagent won't be in the registry anymore).
const SUBAGENT_DISMISS_MS = 3000;
/** Per-requestId timeout handles, so a fresh terminal event resets
 *  the timer (e.g. error → done within the window). */
const dismissTimers = new Map<string, ReturnType<typeof setTimeout>>();
function scheduleAutoDismiss(requestId: string): void {
  const existing = dismissTimers.get(requestId);
  if (existing) clearTimeout(existing);
  const handle = setTimeout(() => {
    dismissTimers.delete(requestId);
    const st = useStore.getState();
    if (st.subagents.find((r) => r.requestId === requestId)) {
      st.removeSubagent(requestId);
    }
  }, SUBAGENT_DISMISS_MS);
  dismissTimers.set(requestId, handle);
}
/** Called when the registry is wiped (next orchestrator turn) so
 *  the pending timers don't fire on already-removed entries. */
function clearAllDismissTimers(): void {
  for (const handle of dismissTimers.values()) clearTimeout(handle);
  dismissTimers.clear();
}

// ── Helpers used by the SubagentWindow component ──

/** Cancel a working subagent. The backend treats `sub-<uuid>` as a
 *  normal request id; the existing `cancel_agent` IPC works. */
export function cancelSubagent(requestId: string): void {
  ipc.cancelAgent(requestId).catch(() => {});
}

/** Respond to a pending tool approval surfaced in a subagent viewer.
 *  Clears the window's own approval card (separate from the
 *  orchestrator composer's `pendingApproval`) and dispatches the
 *  answer via the shared `respond_approval` IPC. */
export function respondSubagentApproval(
  requestId: string,
  allow: boolean,
): void {
  const w = useStore.getState().subagentWindows[requestId];
  if (!w?.approval) return;
  useStore.getState().setSubagentApproval(requestId, null);
  ipc.respondApproval(requestId, allow).catch(() => {});
}
