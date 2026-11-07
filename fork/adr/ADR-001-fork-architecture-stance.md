---
type: adr
id: FORK-ADR-001
status: accepted
reversal_cost: Medium
filed: 2026-11-08
date: 2026-11-08
---

# FORK-ADR-001: Where fork changes live, and how they go upstream

**Date:** 2026-11-08
**Status:** Accepted
**Deciders:** Abhijit Salvi (fork owner)

## Context

Upstream `AGENTS.md` keeps the app's core small: the model, machine layout documents, commands and shared mod APIs. A change belongs there only when all three answers are yes, and then only with the maintainer's (Can's) go:

1. Does the app itself act on it?
2. Can only the app see it?
3. Can mods not do it correctly, even with care?

Otherwise it belongs in mods, "even at the price of some repetition". herdr-core changes go through Can too.

The fork is a daily driver first, and grew fast. The audit of 2026-11-08 (`fork/upstream-audit-2026-11-08.md`) found:

- 67 changes in all;
- **16** that could go upstream as they are;
- **8** that could go upstream after reshaping;
- **28** that need Can's go;
- **13** that stay fork-only;
- **2** already merged upstream.

About 99% of the changed lines are in herdr-app, mostly Rust. The four herdr-core changes are fixes. Thirteen features bend the rule: they compute display facts in Rust that mods could compute. Examples are pane labels, automatic pane names, icons, header info, toast colours, the working spinner, space avatar and dock badge. Recent feature branches were also cut from the fork's `master` rather than `upstream/master`, so they can't be offered as they are.

## Decision

**Where a fork change goes:**

- **App Rust core** when the three questions hold: rendering and terminal drawing, OS integration (notifications, Dock, app icon, focus), and commands the app acts on (moving tabs, arranging panes, copy, menus). Fixes in any layer qualify.
- **Mods** (shipped `defaults/` or the owner's personal mods) for anything that only changes how something looks or reads: labels, names, icons, colours, templates and menu entries. Repetition across mods is acceptable.
- **Fork-only Rust for display facts** is allowed when it buys a measured win (one git cache for every mod, header info resolved once per change rather than per render). It stays fork-only, it's marked as rule-bending in its `FORK.md` section, and it is reshaped into mods (or a smaller core surface such as raw facts on `panes()`) before it's offered upstream. The audit's "Rule-bending features" table is the reshape plan.
- **herdr-core** gets fixes only. A new core API the app needs is an upstream conversation first.

**Every new fork feature records its three answers.** These go in its `FORK.md` section and its pull request body, with an upstream verdict: A as is, B reshape, C ask Can, or D fork-only.

**Branching.**

- A change meant for upstream branches from `upstream/master` (the `FORK.md` workflow).
- A change that depends on other fork features may branch from `master`. Its `FORK.md` section then says so, and offering it upstream means cutting a fresh branch from `upstream/master` and re-applying it.

**Upstream order:**

1. **Wave 0:** land the PRs already open upstream.
2. **Wave 1:** open the A-verdict fixes. These include atomic writes keeping file modes, changed-rows-only, the keys fix, fallback fit, and the three herdr-core fixes.
3. **Wave 2:** C proposals, each with an issue first.
4. **Wave 3:** the rendering settings keys, asked for once as a batch.

Fork-only changes are never offered.

## Consequences

### Positive

- The daily driver keeps its features while the path upstream stays clear and ranked.
- Each feature carries its own justification, so a later reviewer, or Can, sees the reasoning without a fresh audit.
- Fixes reach upstream first, which is where they help most and cost least to review.

### Negative / trade-offs

- **Rule-bending features cost twice:** once built in Rust for the fork, and again reshaped into mods to go upstream.
- **Weekly syncs keep conflicting** in the large Rust files: `src/window.rs`, `src/settings.rs`, and the `vendor/` GPUI patches.
- **Many new settings keys** (about 40) are compatibility promises the fork carries alone until upstream takes them.

### Neutral

- The owner's personal mods (chezmoi `dot_config/herdr-app/mods/`) are the natural home for presentation that upstream would not ship. They are not upstream candidates.

## Alternatives considered

### Alternative A: follow upstream's rule strictly in the fork

Rejected for now. Some display features need data that mods can't reach today, such as `gitStatus` and session names. Others measurably cost more in mods, such as per-render template evaluation. Reshaping them all first would stall the daily driver.

### Alternative B: stop upstreaming and run the fork as its own product

Rejected. Upstream moves fast, and every divergence raises the sync cost. Fixes belong upstream, and the owner already has issues and PRs open there.
