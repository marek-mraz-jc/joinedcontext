## What

<!-- One paragraph. Clause-prefixed title for normative work (`5.6.6: ...`). -->

## Proof

- [ ] Base branch is `dev` (only a release promotion targets `main`)
- [ ] Targeted tests green (`cargo test -p <crate> <filter> -j 2`) — paste the tail
- [ ] Normative change: clause's Robot TPs green against a local broker (CONTRIBUTING recipe)
- [ ] New test seen red first (test-first) or one invert→fail→restore cycle
- [ ] Commits signed off (`git commit -s`, DCO)
