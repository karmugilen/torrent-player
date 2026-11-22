# Sync desktop from Android

Run an on-demand desktop sync from Android commits on `origin/karmugilen/rust-engine`.

1. From the repo root, run a dry run and show the user what would be ported:

```sh
scripts/desktop-sync.sh --dry-run
```

2. If the dry run reports commits to port, run the real sync (creates a
   `desktop-sync/<date>` worktree/branch, runs headless Claude there, notifies
   when ready). Do not merge or push:

```sh
scripts/desktop-sync.sh
```

3. Report the dry-run output, the branch name, the worktree path, and that the
   branch is waiting for the user to review and merge. If the dry run said
   nothing to port, stop after showing that — do not run the real sync.
