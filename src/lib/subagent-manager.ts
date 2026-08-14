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

/** When a subagent reaches a terminal state, check whether any
 *  other subagent is still working. If not, the tiling region
 *  collapses (App's `hasLiveWork` flips to false) and the chat
 *  reflows back to its original full-width / centered position —
 *  this helper additionally clears the locked `chatRatio` so the
 *  next spawn starts from scratch instead of inheriting a stale
 *  lock. Safe to call on every terminal event: it only acts when
 *  the registry has zero working entries. */
function reflowChatIfIdle(): void {
  const st = useStore.getState();
  const stillWorking = st.subagents.some((s) => s.status === "working");
  if (!stillWorking) {
    st.clearChatRatio();
  }
}

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
          // If this was the last live turn, reflow the chat back to
          // its original full-width / centered position.
          reflowChatIfIdle();
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
          // Same reflow trigger as the done path: a cancelled or
          // errored subagent is no longer working. If it was the
          // last live one, the chat reflows.
          reflowChatIfIdle();
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
        // Lock the chat ratio ONLY if a working subagent is still in
        // the registry. A snapshot of all-terminal records is the
        // "all subagents terminated" state the user spec calls out:
        // the chat should reflow back to its original full-width /
        // centered position immediately on reload, not wait for the
        // user to close the (now read-only) windows.
        const hasLive = list.some((r) => r.status === "working");
        if (hasLive) {
          if (st.chatRatio === null) st.setChatRatio(DEFAULT_CHAT_RATIO);
        } else {
          st.clearChatRatio();
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
