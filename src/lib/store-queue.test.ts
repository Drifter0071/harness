import { describe, expect, it, beforeEach } from "vitest";
import { useStore } from "./store";

// Queue semantics used by the composer: FIFO enqueue while a generation
// runs, front-insert for force-send, discard by id, inline edit in place.
describe("message queue store", () => {
  beforeEach(() => {
    useStore.setState({ pendingMessages: [] });
  });

  it("enqueue appends drafts in FIFO order", () => {
    const st = useStore.getState();
    st.enqueueMessage({ id: "a", content: "first" });
    st.enqueueMessage({ id: "b", content: "second" });
    expect(useStore.getState().pendingMessages.map((m) => m.id)).toEqual(["a", "b"]);
  });

  it("enqueue atFront jumps ahead of already-queued drafts (force-send)", () => {
    const st = useStore.getState();
    st.enqueueMessage({ id: "a", content: "first" });
    st.enqueueMessage({ id: "b", content: "forced" }, true);
    expect(useStore.getState().pendingMessages.map((m) => m.id)).toEqual(["b", "a"]);
  });

  it("removeQueuedMessage discards a draft by id", () => {
    const st = useStore.getState();
    st.enqueueMessage({ id: "a", content: "first" });
    st.enqueueMessage({ id: "b", content: "second" });
    st.removeQueuedMessage("a");
    expect(useStore.getState().pendingMessages.map((m) => m.id)).toEqual(["b"]);
  });

  it("updateQueuedMessage edits content (and mentions) in place", () => {
    const st = useStore.getState();
    st.enqueueMessage({ id: "a", content: "old" });
    st.updateQueuedMessage("a", { content: "new", mentions: [{ path: "x.ts" }] });
    const msg = useStore.getState().pendingMessages[0];
    expect(msg?.content).toBe("new");
    expect(msg?.mentions).toEqual([{ path: "x.ts" }]);
  });

  it("clearPendingMessages empties the queue", () => {
    const st = useStore.getState();
    st.enqueueMessage({ id: "a", content: "first" });
    st.clearPendingMessages();
    expect(useStore.getState().pendingMessages).toEqual([]);
  });
});
