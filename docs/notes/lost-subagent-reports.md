# Lesson: a sub-agent's completion notice can arrive without its work

During M3-B the implementation agent hit the account session limit; its
completion notification contained only the limit notice — no report — and
`git status` showed it had written **zero files** before dying. The failure
mode is silent: the orchestrator gets a "finished" signal that looks like a
lost report, when in fact the work never happened.

Rule: on any agent completion whose report is missing or truncated, do not
assume partial work exists. Check the working tree first (`git status`,
`git diff --stat`); the tree is the only ground truth. If the tree is clean,
re-dispatch the same prompt fresh (the session transcript `.jsonl` retains
the original dispatch prompt verbatim — recover it from there rather than
reconstructing from memory).
