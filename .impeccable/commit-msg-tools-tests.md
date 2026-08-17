feat(orchestrator): plural tool tests + system prompt hint

- Add 5 new tests for `delegate_subagents`: role gate, missing/empty
  array rejection, all-good batch, partial-success with per-entry
  failures, all-bad batch. Cover the new `Vec<String>` index shape.
- Update orchestrator system prompt to prefer `delegate_subagents`
  for 2+ independent dispatches and explain when to use the
  singular tool instead.
- Apply `cargo fmt` to the plural tool's helper functions.
