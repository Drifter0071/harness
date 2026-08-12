import { describe, expect, it } from "vitest";
import { chatErrorActions } from "./chat-errors";
import type { ChatError } from "./ipc";

const err = (kind: ChatError["kind"], extra?: Partial<ChatError>): ChatError => ({
  requestId: "r1",
  kind,
  error: "raw provider message",
  ...extra,
});

describe("chatErrorActions", () => {
  it("cancelled → silent, remove the optimistic message", () => {
    const a = chatErrorActions(err("cancelled"));
    expect(a.toast).toBe("");
    expect(a.bubble).toBeNull();
    expect(a.reenableComposer).toBe(true);
  });

  it("auth → toast + bubble, composer falls back to no-key", () => {
    const a = chatErrorActions(err("auth"));
    expect(a.toast).toContain("Invalid API key");
    expect(a.bubble).toBe("(auth failed: re-add API key)");
    expect(a.reenableComposer).toBe(false);
  });

  it("rate_limit without retry-after → generic toast", () => {
    const a = chatErrorActions(err("rate_limit"));
    expect(a.toast).toContain("Rate limited");
    expect(a.bubble).toBe("(rate limited)");
  });

  it("rate_limit with retry-after → includes the seconds", () => {
    const a = chatErrorActions(err("rate_limit", { retryAfterSeconds: 42 }));
    expect(a.toast).toContain("42s");
    expect(a.bubble).toContain("42s");
  });

  it("network → raw message in the bubble", () => {
    const a = chatErrorActions(err("network"));
    expect(a.bubble).toBe("raw provider message");
    expect(a.toast).toBe("Network error — please retry.");
  });

  it("provider → generic provider bubble", () => {
    const a = chatErrorActions(err("provider"));
    expect(a.bubble).toBe("(provider error)");
  });

  it("bad_request → truncates long bodies to 200 chars", () => {
    const long = "x".repeat(500);
    const a = chatErrorActions(err("bad_request", { error: long }));
    expect(a.bubble!.length).toBeLessThanOrEqual(203);
    expect(a.bubble!.endsWith("...")).toBe(true);
  });

  it("internal → generic internal bubble", () => {
    const a = chatErrorActions(err("internal"));
    expect(a.bubble).toBe("(internal error)");
  });
});
