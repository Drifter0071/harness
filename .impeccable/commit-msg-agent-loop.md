feat(orchestrator): wire delegate_subagents into the batch dispatcher

- Dispatch branch now calls the registration helpers directly instead
  of relying on execute_one to populate dispatch_call_index. This
  closes a registration gap in the original parallel-subagent code
  (the previous execute_tool_batch read from the index without ever
  populating it for delegate_subagent calls) and lets the plural tool
  fan one model-side call into N subagent rows under the same call id.
- Per-call agent-tool-start / agent-tool-result events and
  push_activity rows are emitted manually so the UI's tool log still
  shows the per-dispatch "queued subagent ..." entries.
- The final synthetic tool message is deduped by call id so the
  plural tool gets exactly one role: "tool" message per tool_call,
  regardless of fan-out. The combined summary covers all dispatched
  subagents.
