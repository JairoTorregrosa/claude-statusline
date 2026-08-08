# Governance

This document makes agent-mediated contributions reviewable. The rules
live in machine-readable form in [agm.json](agm.json); this file explains
them for humans.

## Why

Agents make contributions cheap to produce. They do not make
contributions cheap to verify. One maintainer reviews every change; a
review is only as fast as the information in front of it. These rules
move the preparation of that information to the contributor and the
contributor's agent, and keep the decision with the maintainer.

The principle is the same one the statusline itself follows: absence must
be declared, not silent. The statusline never renders a partial sum as a
total and never invents a denominator. A contribution never presents an
unverified assumption as a verified one — it declares what was checked
and what was not.

Three files divide the work. [AGENTS.md](AGENTS.md) tells an agent how to
build and modify the project. Commit trailers record which tool produced
a change. This document and [agm.json](agm.json) define what a change
must prove before review.

## Risk zones

Every file belongs to a zone. A change's zone is the highest zone among
its changed files.

| Zone | Files | Why |
|---|---|---|
| critical | `install.sh`, `.github/*`, `AGENTS.md`, `GOVERNANCE.md`, `agm.json` | Writes to user machines, publishes binaries, instructs agents, or changes the rules of review itself. |
| high | `main.rs`, `git.rs`, `cache.rs`, `sessions.rs`, `settings.rs`, `Cargo.toml`, `Cargo.lock` | Process spawns, filesystem access, cross-session state, dependency supply chain, loud-degradation semantics. |
| medium | the rest of `src/` | Correctness of displayed numbers. A wrong number is worse than no number. |
| low | documentation, assets | No runtime effect. |

## Evidence packages

The pull-request body carries the evidence. The required sections grow
with the zone:

| Zone | Required sections |
|---|---|
| low | none — open the PR and CI does the rest |
| medium | Summary · Checks · Behavior evidence |
| high | + External assumptions · Risk |
| critical | + Second review · Human confirmation |

**External assumptions** is the section that tests cannot replace. This
project reads data it does not control: the stdin payload, the transcript
JSONL, `settings.json`, git output. CI has no real transcripts, so a
change built on a wrong schema belief passes CI and still lies on screen.
State each belief and how you verified it against real data.

**Second review** means an adversarial pass by a second agent or a human:
someone whose task is to refute the change, with findings and their
resolution recorded.

**Human confirmation** is a checkbox only the human contributor sets. An
agent prepares the package; it never confirms it. This is the boundary
between preparation and responsibility.

## Gates

The `AGM` workflow enforces the mechanical gates on every pull request:
it computes the zone from the changed files, compares it with the
declared zone, and checks that the required sections and the
confirmation box are present. Its job summary is the review packet: zone,
evidence status, missing items.

The final gate — approval — belongs to the maintainer. No tool sets it,
no contributor statement substitutes for it, and a green `AGM` check does
not imply it.

## Proportionality

A low-risk change carries no added burden: fix a typo, open the PR, done.
The obligations concentrate where a wrong change hurts: the installer
that edits `~/.claude/settings.json`, the workflows that publish
binaries, the code that spawns processes or holds cross-session state.
For changes that need assurance beyond these rules (signed commits,
protected branches), the maintainer can add platform controls on top;
this document does not replace them.
