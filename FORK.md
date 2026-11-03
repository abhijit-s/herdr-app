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
3. Merge and check it on `master`: `git switch master && git merge --no-ff feat/<slug> && mise exec -- just check`. Upstream's code is not `cargo fmt` clean, so match the surrounding style rather than running `cargo fmt`.
4. Try it: `mise exec -- just install`, then restart Herdr.
5. Push the branch: `git push -u origin feat/<slug>`. Bigger changes get an upstream issue first (upstream `CONTRIBUTING.md`).
6. Open the pull request: `gh pr create -R herdrdev/herdr-app --head abhijit-s:feat/<slug>`.
7. Keep it current when upstream moves: `git rebase upstream/master feat/<slug> && git push -f origin feat/<slug>`.
8. Sync and reinstall: `scripts/fork-sync`. It lists branches whose commits are not yet upstream. Once a change lands upstream, delete its branch and its entry below.

## Changes

### Terminal line height

Upstream pull request #67 (`pr/67`, by juanibiapina, closes #66), merged here as is: `line_height` in settings.json or a layout's look, the row height as a multiple of the font's height (0.8 to 2, default 1.15). It replaced this fork's own `feat/line-height`, which supported settings.json only.

- Settings window: a "Terminal line height" stepper (0.05 steps) and a "Terminal ligatures" toggle for upstream's `font_ligatures`, which had no row. Both are committed on `master` only, with a render test for all the terminal rows (`terminal_text_rows_show_whose_value_is_in_use`).
- Files: `src/settings.rs`, `src/layout.rs`, `src/terminal.rs` (`cell_size`), `defaults/AGENTS.md`; on `master`: `defaults/frame/settings/main.js`, `src/mods/render_tests.rs`
- Upstream: pull request #67 (not ours), awaiting review

### Terminal font weight

`feat/font-weight`: `font_weight` in settings.json or a layout's look (100 to 900, default 500, the Medium upstream hard-codes); bold stays at least 700. Rows kept from earlier frames are keyed on the whole cell font, so a weight change redraws them.

- Settings window: a "Terminal font weight" stepper (steps of 100) under the terminal font. When the layout's look sets a weight, the row says so, as the font row does.
- Files: `src/settings.rs`, `src/layout.rs`, `src/terminal.rs` (`cell_font`, `paint_grid`), `defaults/AGENTS.md`, `defaults/frame/settings/main.js`
- Upstream: issue #134, pull request #332. Upstream's `font_ligatures` (#244) landed alongside it; the branch merges `upstream/master` to stay conflict-free.

### Cell width and letter spacing

`feat/terminal-letter-spacing` (issue #171, PR #336): `letter_spacing`, px at zoom 1 added to the cell width (-2 to 10, default 0), set in settings.json or a layout's look. The cell stays within half to one and a half of the font's advance, because gpui-pre's force-width pass treats a glyph less than half a cell past the previous letter as a combining mark. Needs a `HERDR:` change in `vendor/gpui-pre/src/text_system/line_layout.rs` (`apply_force_width_to_layout`): base glyphs always sit on their cells, where a 1px tolerance made text creep then jump whenever the cell width differs from the font's advance. gpui-pre's own tests don't build in this checkout, so the placement rule was checked with a standalone copy of the function.

`feat/cell-width-pixel` (on top of `feat/terminal-letter-spacing`, fork only for now): `cell_width` (`"font"`, default, or `"pixel"`: the spaced width rounded down to whole device pixels, as Ghostty does). `feat/letter-spacing` is the retired first version of both.

- Settings window: a "Terminal letter spacing" stepper (0.5px steps) on `feat/terminal-letter-spacing`, and a "Terminal cell width" choice (Font or Whole pixels) on `feat/cell-width-pixel`, which turns the appearance picker into a general `choicePicker`. `feat/cell-width-pixel` merges `feat/terminal-letter-spacing` to pick up its row.
- Files: `src/settings.rs`, `src/terminal.rs` (`cell_size`), `vendor/gpui-pre/src/text_system/line_layout.rs`, `defaults/AGENTS.md`, `defaults/frame/settings/main.js`
- Upstream: issue #171, pull request #336 (`feat/terminal-letter-spacing`); `feat/cell-width-pixel` is fork only

### Variable font weights

`fix/variable-font-weight`: draws a variable face at the requested weight. Herdr bundles JetBrains Mono as one variable file, so its family has a single face and every weight (Medium text, Bold, `font_weight`) drew at the default instance. A `HERDR:` change in `vendor/gpui-pre-macos/src/text_system.rs` (`at_weight`, `variation_weight`) copies the face at the asked weight on its `wght` axis and maps CoreText's runs back to that copy by PostScript name and weight. Desk-verified: bundled JetBrains Mono at 225 matches Rex's 225 (same grid, ink within 4%).

- Files: `vendor/gpui-pre-macos/src/text_system.rs`
- Upstream: pull request #333 (on macOS the bundled JetBrains Mono drew Medium and Bold at Regular)


### Pane names

`feat/pane-rename` (for issue #322, pull request not yet opened): `pane.rename {pane?, name}` names a pane like `tab.rename` and `space.rename`, asking with the shipped prompt when there's no name; an empty name goes back to the program's own. The name is the pane block's `title` in its tab's tree, so it saves and syncs with the layout document. `panes()` exposes it as `name`, and the shipped `pane` and `terminals-pane` headers show it.

- Files: `src/commands.rs`, `src/window.rs`, `src/model.rs`, `src/mirror.rs` (tests), `defaults/AGENTS.md`, `defaults/herdr.d.ts`, `defaults/mods/pane/main.js`, `defaults/mods/terminals-pane/main.js`, `docs/commands.md`
- Upstream: issue #322 asks for it (as `terminal.rename`)
