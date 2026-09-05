# TODO, moved

This file no longer holds the backlog. Its contents were split into issues, one
per limitation, on 2026-08-27.

- Known limitations, summarized for users: [limitations.md](limitations.md)
- Open work, one entry per limitation:
  [the tracker](https://github.com/rboudrouss/reactant-analyzer/issues)

Labels: `soundness-bug` (the analysis is wrong, not merely imprecise),
`precision-fn` and `precision-fp` (accepted trade-offs), `infra`,
`rule-proposal`, `ux`. Size is `size/S|M|L`, and `blocked` marks an issue
waiting on another.

Limitations settled as "we are not fixing this" are **closed** `wontfix`
issues, so the reasoning stays citable and nobody proposes the fix again:
[`--state closed --label wontfix`](https://github.com/rboudrouss/reactant-analyzer/issues?q=is%3Aissue+is%3Aclosed+label%3Awontfix).

This file stays as a redirect because a dozen ADRs cite it by this path, and an
ADR is a historical record that does not get rewritten.
