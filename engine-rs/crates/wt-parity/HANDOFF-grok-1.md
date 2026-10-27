# HANDOFF — grok-1 (wt-parity)

Paused with types.rs JSON Go-compat task. See detailed note:

`engine-rs/crates/wt-engine/src/types.HANDOFF-grok-1.md`

## wt-parity status

- `api-diff` CLI done; report at `API_DIFF.md` / `API_DIFF_CRITICAL.md`.
- After types + agy-4 call-sites: re-run api-diff, refresh counts, classify remaining as server/core.

## Verify

```bash
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-parity CARGO_BUILD_JOBS=3 \
  cargo clippy -p wt-parity -- -D warnings
# then api-diff as in types HANDOFF
```
