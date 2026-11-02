# Fork changes

This fork of [herdrdev/herdr-app](https://github.com/herdrdev/herdr-app) is the Herdr build used day to day. It carries small, focused changes that each go upstream as their own pull request, and it tracks upstream by merging `upstream/master` into `master`.

This file is the inventory of what diverges. Update it in the same commit that adds, removes or upstreams a change.

## Branches

- `master`: `upstream/master` plus every change below, merged in. This is what gets built and installed.
- `feat/<slug>`, `fix/<slug>`: one change each, branched from `upstream/master` so its pull request carries nothing else.
- `FORK.md`, `scripts/fork-sync` and `mise.local.toml` (untracked) exist only on `master`.

## Setup

- Toolchains match upstream CI: Rust 1.95.0 and Zig 0.16.0, pinned per folder in `mise.local.toml` (listed in `.git/info/exclude`). Run commands through `mise exec --`.
- `just install` builds a release into `~/Applications/Herdr.app` and points `~/.local/bin/herdr-app` at it. Quit and reopen Herdr to use it. herdr-core is installed separately and is not rebuilt.
- `just notices` needs cargo-about 0.9.3: `cargo install --locked cargo-about@0.9.3 --features cli --root ~/.local/share/herdr-app-tools`, found through `CARGO_ABOUT` in `mise.local.toml`.
- App home stays `~/.config/herdr-app`, shared with the release build.

## Workflow

1. Start a change: `git switch -c feat/<slug> upstream/master`.
2. Make it, gated behind a setting wherever practical, and commit.
3. Merge and check it on `master`, which has the Xcode build fix branches lack: `git switch master && git merge --no-ff feat/<slug> && mise exec -- just check`. Upstream's code is not `cargo fmt` clean, so match the surrounding style rather than running `cargo fmt`.
4. Try it: `mise exec -- just install`, then restart Herdr.
5. Push the branch: `git push -u origin feat/<slug>`. Bigger changes get an upstream issue first (upstream `CONTRIBUTING.md`).
6. Open the pull request: `gh pr create -R herdrdev/herdr-app --head abhijit-s:feat/<slug>`.
7. Keep it current when upstream moves: `git rebase upstream/master feat/<slug> && git push -f origin feat/<slug>`.
8. Sync and reinstall: `scripts/fork-sync`. It lists branches whose commits are not yet upstream. Once a change lands upstream, delete its branch and its entry below.

## Changes

### Build without Xcode

`fix/build-without-xcode`: passes `-Demit-xcframework=false` to Ghostty's lib-vt build. Ghostty builds the xcframework whenever `xcodebuild` is on PATH, and with only the Command Line Tools `/usr/bin/xcodebuild` is a stub that fails.

- Files: `crates/ghostty-sys/build.rs`, `THIRD-PARTY-NOTICES`
- Upstream: not yet proposed

### Terminal line height

`feat/line-height`: `line_height` setting, the row height as a multiple of the font's ascent plus descent (1 to 2, default 1.15, the value upstream hard-codes).

- Files: `src/settings.rs`, `src/terminal.rs` (`cell_size`), `defaults/AGENTS.md`
- Upstream: issue #66, pull request not yet opened

### Terminal font weight

`feat/font-weight`: `font_weight` setting for normal terminal text (100 to 900, default 500, the Medium upstream hard-codes); bold stays at least 700.

- Files: `src/settings.rs`, `src/terminal.rs` (`cell_font`), `defaults/AGENTS.md`
- Upstream: issue #134, pull request not yet opened

### Coding ligatures

`feat/ligatures`: `ligatures` setting that turns the font's `calt` feature back on (one glyph per cell in coding fonts); `liga` and `dlig` stay off.

- Files: `src/settings.rs`, `src/terminal.rs` (`cell_font`), `defaults/AGENTS.md`
- Upstream: issue #176, pull request not yet opened
