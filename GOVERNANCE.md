# Governance

A pull request declares what produced it. That is the whole rule. The
machine-readable form is [agm.json](agm.json); the `AGM` check enforces
it.

## The declaration

Four lines in the pull-request body:

```
- Model: Opus 5 (1M context), Codex
- Harness: Claude Code 2.1
- Tokens: 812000
- Cost: 0 (subscription, no metered spend)
```

| Field | Means |
|---|---|
| Model | Every model that wrote part of the change. List all of them. |
| Harness | The tool they ran in, with a version when you have one. |
| Tokens | Total for the session. Rounded is fine. |
| Cost | USD. A flat-rate subscription with no metered spend is 0. |

A change written without an agent declares `none`, `none`, `0`, `0`. A
number the harness does not report is `unknown` — declared ignorance is
accepted, an invented number is not.

Commits keep the agent's `Co-Authored-By` trailer.

## Why this and nothing else

Agents make contributions cheap to produce and no cheaper to verify. The
earlier version of this document answered that with risk zones and
evidence packages sized to the files a change touched. It asked the
contributor to pre-compute the maintainer's judgment, and the checklist
grew faster than the trust it bought.

What survives is the part a maintainer cannot reconstruct after the
fact: which models wrote this, in what harness, and what it cost to
produce. A diff shows what changed. Only the contributor knows what
produced it, and that context changes how a review reads — an unverified
claim from a model that ran for eight hundred thousand tokens is a
different object from a typo fix.

Everything else moved to where it belongs. Correctness is CI's job:
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` on Linux, macOS and Windows. Finding what CI cannot is
Codex's job; its review rules live in
[AGENTS.md](AGENTS.md#code-review-rules). Deciding is the maintainer's
job.

## Gates

`AGM` checks that the four lines are present and carry values. It cannot
verify what they say, and does not try. The declaration is the
contributor's word, on the record — a false one is a lie, not a gate
failure.

Merging belongs to the maintainer. A green `AGM` check is not approval,
and neither is a Codex review with no findings.
