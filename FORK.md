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

- `cell_size` clamps the value to 0.8 to 2 (`row_height`), on `master` only: the pull request only reports a value out of range, and `0` made empty rows and a hang.
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

### Font thickening

`feat/font-thicken`: `font_thicken` and `font_thicken_strength` (0 to 255, default 255), person settings that rasterise terminal glyphs as Ghostty's `font-thicken` does: CoreGraphics font smoothing on, a gray fill of strength / 255 in linear gray, whatever the text color, then tinted. GPUI otherwise picks the smoothing level from the text color's luminance, so dark text on light draws at the lightest level and light text on dark at the heaviest. Panels keep GPUI's behaviour. `HERDR:` changes in `vendor/gpui-pre` (`RenderGlyphParams::thicken`, in the glyph atlas key; `Window::with_glyph_thicken`) and `vendor/gpui-pre-macos/src/text_system.rs` (the rasteriser, and 1 more pixel of raster room, as Ghostty pads thickened glyphs). The terminal paints its grid inside `with_glyph_thicken`, and the row cache's `painted_with` key carries the strength.

- Measured (`tests/thicken_ink.rs`, JetBrains Mono 12px at 1x): font smoothing steps at strengths about 112, 192, 224 and 255, so Ghostty's 30 is its lightest level. Weight 400 with strength 30 inks both polarities alike (0.126), as Ghostty does; weight 250 without thickening inks light on dark 0.134 and dark on light 0.107.
- Files: `src/settings.rs`, `src/terminal.rs` (`glyph_thicken`, `paint_grid`), `vendor/gpui-pre/src/{text_system.rs,window.rs}`, `vendor/gpui-pre-macos/src/text_system.rs`, `vendor/gpui-pre-{wgpu,windows}` (the new field), `tests/thicken_ink.rs`, `defaults/AGENTS.md`
- Upstream: fork only for now


### Pane names

`feat/pane-rename` (for issue #322, pull request #384): `pane.rename {pane?, name}` names a pane like `tab.rename` and `space.rename`, asking with the shipped prompt when there's no name; an empty name goes back to the program's own. The name is the pane block's `title` in its tab's tree, so it saves and syncs with the layout document. `panes()` exposes it as `name`, and the shipped `pane` and `terminals-pane` headers show it.

- Files: `src/commands.rs`, `src/window.rs`, `src/model.rs`, `src/mirror.rs` (tests), `defaults/AGENTS.md`, `defaults/herdr.d.ts`, `defaults/mods/pane/main.js`, `defaults/mods/terminals-pane/main.js`, `docs/commands.md`
- Upstream: pull request #384, for issue #322 (which asks for `terminal.rename`)

### Toasts fit their content

`feat/toast-fit-content` (branched from `master`, fork only): a toast is as wide as its content, 200 to 340 px (titles still truncate at 340), and 40 px high without a body, 72 with one (a body as the toast mod draws one: text that isn't blank, or any other value JavaScript calls true); an explicit `height` still wins, and toasts stay right-anchored and stacked. `fix/toast-content-fill`: the toast mod's root fills the pill (inside its border) and its text is vertically centred; a fitted `ShellRoot` grows to its container when `set_fill_fitted` is on, which only toasts use, so fitted popups and floats are unchanged. A keyed (`.id(...)`) script element records its bounds under that id in debug test builds, so tests can measure mod content.

- Files: `src/window.rs` (`toast_height`, `open_overlay`, `render_overlays`, tests `toast_sizes` and `a_toast_is_two_lines_high_only_with_a_body_the_mod_draws`), `defaults/mods/toast/main.js` (`hasBody`), `vendor/gpui-kit/crates/shell/src/root.rs` (`fill_fitted`), `vendor/gpui-kit/crates/shell/src/materialize.rs` (debug selector for keyed elements), `defaults/AGENTS.md`
- Upstream: none yet

### Popup command

`feat/popup-command` (branched from `master`, fork only): `terminal.popup {argv, cwd?, hold?, wait?, width?, height?}` starts a program on this computer as `hc run --bg --no-parent` does (`rm` unless `wait`) and opens the `terminal.peek` overlay for it in the same step, so there is no retry while the window learns about the terminal. `herdr-app popup [--cwd DIR] [--hold] [--wait] [--size WxH] [--] cmd args…` drives it over the control socket; with `--wait` it sends `{"wait": terminal}`, which the window answers from the core's events once the terminal finishes, then removes the terminal, and the CLI exits with its exit code, 128 + the signal for a program a signal killed (the core's `signal`), or 1 (removed before it ended, the window is not open or the popup failed). The popup's ✕ and backdrop only hide it and the program keeps running: `POPUP_CLOSE_ENDS_PROGRAM` in `src/window.rs` flips that, through the peek bar's `close_ends_program` prop. A new popup over an open one ends the program the open one started with `terminal.popup` (`peeked` tracks it; a `terminal.peek` of an existing terminal, or a hidden popup, is never ended, and a plain `terminal.peek` over an open popup, such as a link clicked in it, only hides that popup). It replaces the logic of the `herdr-popup` shell script in dotfiles, which is now a wrapper.

- `--hold` reads one key with `stty` and `dd`, which any `sh` has (dash, Linux's `/bin/sh`, has no `read -n`).
- Timeouts: the control server gives `terminal.popup` 15 s (`POPUP_ANSWER_WITHIN`), past the 10 s `Core::call` may take for `terminal.run` on the UI thread, and the CLI waits 2 s longer. If the CLI gives up anyway, the reply channel is gone when the window answers, and the window ends and removes the wait popup (`abandon_popup`) instead of keeping it.
- While a request waits, the server checks its stream for a hang-up every 500 ms (`await_reply`, `hung_up`): Ctrl-C on `--wait` drops the waiter, and the program keeps running. On Unix a hang-up is `POLLHUP` (asked for with `POLLOUT`, which macOS needs to report it), so a client that only shut its writing half after the line (`printf … | nc -U`) is still waited for and gets its answer. A finished wait popup nobody has asked about keeps its answer and its terminal for `WAIT_KEPT` (60 s), then both go (`expire_waits`); closing or quitting the window removes the finished ones (`rm_finished_waits`).
- `--size`: a side up to 1 is a fraction of the window and one from 50 is px; between them (`1.5`) is a usage error, more likely a mistyped fraction than a tiny popup.
- Files: `src/commands.rs`, `src/window.rs` (`popup`, `open_peek`, `abandon_popup`, `end_terminal`, `wait_for`, `settle_waits`, `expire_waits`, `rm_finished_waits`, `popup_hold_argv`, test `popup_peeks`), `src/control.rs` (`popup`, `parse_popup`, `exit_status`, `serve`, `await_reply`, `hung_up`, tests), `src/hc.rs` (`Core::start`, `EventLines::ended`, test core `answering`), `src/main.rs`, `defaults/mods/peek/main.js`, `defaults/AGENTS.md`, `docs/commands.md`
- Upstream: none yet

### System notifications

`feat/system-notifications` (branched from `master`, fork only): an agent alert ("<name> needs you", "<name> finished") also goes to the system's notification center through GPUI's `show_system_notification`, titled as the toast with "space / tab" as the body and tagged `herdr-agent:<address>`, so a newer alert for the same agent replaces the older one. `system_notifications` in settings.json says when: `background` (default; only while Herdr's window isn't key), `always`, or `off`; never while the person is looking at that agent (`alert::should_notify`). Clicking one activates Herdr and runs `agent.focus` for it, as the toast does; it is dismissed once the agent is looked at, starts working again or goes away. `dock_bounce: true` calls `request_attention` for "needs you" while Herdr is in the background. `alert_command` (a string run by `/bin/sh -c`, or an argv array) runs fire-and-forget in place of the system notification, under the same rule, with `HERDR_AGENT_STATE` (blocked|done), `HERDR_AGENT_PREV_STATE`, `HERDR_AGENT_APP`, `HERDR_AGENT_NAME`, `HERDR_AGENT_ADDRESS`, `HERDR_AGENT_PLACE` and `HERDR_ALERT_TITLE`, so the herdr TUI's and tuios's `notify` adapters have a Herdr counterpart. The vendored GPUI posts nothing unless the app runs from a bundle (its `NSInternalInconsistencyException` guard), so dev builds stay silent. Settings window: a "System notifications" choice and a "Bounce the Dock icon" toggle under Alerts.

- An `alert_command` starts in the home folder (the app's own is `/` when opened from the Finder) and in its own process group; still running after 30 s, the whole group is killed (`alert::reap`), so a hung one doesn't keep its thread and programs forever. One that doesn't start posts the system notification instead (`alert::Sent::Fallback`), and an error toast says why, once per distinct reason (`alert_command_errors`, cleared when a settings reload changes `alert_command`). A shell line always starts, so a missing program only shows as exit 127: one that started returns `alert::Sent::Ran` with a channel on which the reaper thread sends why a non-zero exit failed (`alert::exit_failure`), and the window then posts the system notification and the same once-per-reason toast. One killed by a signal is only logged at warn level.
- Files: `src/alert.rs`, `src/window.rs` (`alert`, `tend_terminals`, `notified`, the response handler in `AppWindow::new`), `src/settings.rs`, `src/main.rs`, `defaults/frame/settings/main.js`, `defaults/AGENTS.md`
- Upstream: none yet

### Off-screen terminal output

`perf/offscreen-no-redraw` (branched from `master`, fork only): a test, `output_off_screen_keeps_the_screen_and_redraws_nothing`, holding what `TerminalView::ingest` already does: output in a terminal not drawn in the window's current frame (another tab or space, a parked terminal, a hidden overlay) feeds its screen and still reports titles, but doesn't notify the window. The ~8 window draws a second the bench's `idle-busy-bg` shows come from the working agents' spinners in the tabs, pane header and agents mods (`spin(900)`, woken every 100 ms by gpui-kit's `spin_layer`), not from terminal output: GPUI marks every ancestor of a woken view dirty, so each spinner step runs `AppWindow::render` while terminal draws stay at 0.

- Files: `src/terminal.rs` (tests)
- Upstream: none yet

### Rows kept while output scrolls

`perf/changed-rows-only` (branched from `master`, fork only): a terminal row's drawing (backgrounds, shaped text, icons and sprites) is built again only when the row changed, also while output scrolls the screen. libghostty marks every row dirty when the screen scrolls (see "Row Identity" in its `render.h`), so `Vt::frame` read each one as a new row and `paint_grid`, which kept drawings by position, rebuilt the whole screen every frame an agent streamed output (the bench's `agent` scenario: ~1000 rows a second, ~56 a frame). `Vt::frame` now keeps a dirty row's `Rc` when it reads the same as the row with that id last frame, and `keep_rows` finds a row's drawing by its `Rc` wherever it moved. Selection, colors and width are in the cells, so a row selected, recolored or resized reads differently and is built again; the cell, font, fonts and ligature setting still clear every drawing, and with ligatures the cursor's column is part of the key.

- Files: `src/vt.rs` (`Frame`, `Vt::frame`, test `a_row_stays_the_same_rc_only_while_it_reads_the_same`), `src/terminal.rs` (`PaintedRow`, `keep_rows`, `paint_grid`, test `only_rows_that_changed_are_built_again`)
- Upstream: none yet

### Spinners turn without a draw

`perf/spinner-repaint-only` (branched from `master`, fork only): a mod's `svg(...).spin(ms)` turns with no render, layout or paint of anything, its view's or the window's. GPUI marks every ancestor of a notified view dirty and renders the root view on every draw, so the 100 ms wake gpui-kit's `spin_layer` gave its ShellRoot ran `AppWindow::render` and the whole window's prepaint and paint per step (the bench's `idle-busy-bg`: ~8 window draws a second with no terminal drawn). GPUI now has spinning sprites: `Svg::spinning(period)` / `Window::paint_spinning_svg` paint a monochrome sprite whose `pad` slot (which no shader reads) holds its period, so a cached view replaying the sprite replays the spin. `Window::present` turns every spinning sprite of the frame on screen to `gpui::spin_clock_ms()`, and while that frame has one, a timer asks for a present-only frame every `SPIN_STEP` (100 ms, ~10 fps) through the platform's frame source. gpui-kit's `spin` draws such an svg in place, so the hosted spinner layer (`Spin`, `spin_layer`, `build_view`, `Fade` and the inline fallbacks) is gone, and a spinner keeps its true stacking, opacity and clipping. A spinner turns about its drawn sprite's center (within a device pixel of its element's center).

- A step already scheduled when a draw took the spinners away presents nothing; steps wait while the window is hidden (`Window::visibility`) and start again when it is visible.
- `Scene` keeps `has_spinners`, set when a spinning sprite is inserted (cached replay inserts too) and cleared with the scene, so a present without spinners skips the scan.
- `Svg::spinning` works for an `external_path` or inline data svg as well as a `path` one.
- A spinner drawn fully transparent (opacity or colour alpha 0) is left out of the frame, so it doesn't keep the window presenting. `spin(ms)` ignores a non-finite `ms` and clamps the rest to 100 ms – 1 hour; the gpui-kit tests sit outside the workspace, so a root-crate test (`spin_with_an_absurd_period_draws_without_panicking`) hands Infinity, NaN and 1e300 to `spin` through the script bridge (`__apply`; `.spin()` itself rejects them in JavaScript) and renders.
- The turn is not reflected in the sprite's bounds for layer ordering and clipping, so a spinner should be round or symmetric within its box.

- Files: `vendor/gpui-pre/src/scene.rs` (`MonochromeSprite::spins`, `turn`, `Scene::turn_spinners`, `has_spinners`), `vendor/gpui-pre/src/window.rs` (`SPIN_STEP`, `spin_clock_ms`, `paint_spinning_svg`, `present`, `schedule_spin_step`, `refresh_visibility`, `presented_spinners`, `presents`), `vendor/gpui-pre/src/elements/svg.rs` (`Svg::spinning`), `vendor/gpui-pre/src/app/test_context.rs` (`set_asset_source`, `simulate_window_frame_request`), `vendor/gpui-kit/crates/shell/src/materialize.rs`, `vendor/gpui-kit/crates/shell/src/root.rs`, `vendor/gpui-kit/crates/shell/src/view.rs`, `vendor/gpui-kit/crates/shell/src/typings.rs`, `src/window.rs` (comment), `src/mods/lifecycle_tests.rs` (test `spinners_turn_without_drawing_the_window`)
- Upstream: none yet. Upstream's unmerged `fix/spinner-frame-clock` (cdba8fa, ffe8d0f) swaps the 100 ms wake for `window.request_animation_frame()`, which keeps the full window draw per step and steps it every display frame; this replaces the wake with present-only frames instead.

### Interface zoom and text size

`feat/ui-zoom-text-scale` (branched from `master`, fork only): two settings.json scales for the panels, both default 1, where 1 draws exactly as `zoom` alone. `ui_zoom` (0.75 to 1.5) scales the layout's px sizes, pane headers, tabs, overlays, popups, toasts and every mod's px on top of `zoom`; terminals keep their font, cells, letter spacing and padding at `zoom` alone. `ui_text_scale` (0.8 to 1.3) multiplies only text: Herdr's panel text (`settings::zt`, `settings::text_rems` for what was `text_xs`, and the window root's text size), and mods' `text_size(...)` (px or `"1rem"`), the reflected `text_xs()` … `text_3xl()`, px and rem `line_height`s, editor `font_size`, mod tooltips and the shell's failure banner through a `HERDR:` text scale in gpui-kit's bridge (`set_text_scale`, `scaled_text_px`, `scaled_text_rems`, `style::text_length`, `style::apply_nullary`, `style::line_height`). The window's rem (`settings::ui_rem`) is the base text size at `zoom × ui_zoom` without the text scale, so rem spacing, radii and gpui-kit's own panels keep their size. `zoom × ui_zoom` stops at the largest zoom (3). Both are clamped when settings.json is read, reported by `Settings::problems()` when out of range (`asked_ui_scales`), and apply live like the other settings. Mods get `settings().ui_scale` (`zoom × ui_zoom`, capped) and `settings().ui_scale_ranges` (`Settings::for_mods`); mods that turn window px back into their own (`tabs`, `picker`, Herdr 98's `kit.js` and taskbar, What's New) divide by `ui_scale`; Herdr 98's terminal font stays on `zoom`. Rust chrome that used rem spacing (`p_2`, `px_3`, the exit badge) uses `z()` instead, so text scale leaves its spacing alone.

- Settings window: "Interface zoom" and "Interface text size" steppers (5% steps) under Zoom, from the value in use (settings.json's as Herdr clamps it) within the published ranges (`scaleStep`); 100% leaves the key out of settings.json.
- Tests: `cargo test ui_scale` matches only the inner `mod_ui_scales`, which returns at once outside its own process; `mods_follow_ui_zoom_and_text_scale_live` is the one that runs it.
- Files: `src/settings.rs` (`Person`, `Settings::compose`, `ui_scale`, `z`, `zt`, `ui_font_size`, `mod_scales`, tests), `src/terminal.rs` (`pad`, `cell_size_at`, test `ui_scales_leave_terminal_cells_alone`), `src/window.rs`, `src/window/pane_size.rs`, `src/window/floats.rs`, `vendor/gpui-kit/crates/shell/src/value.rs`, `vendor/gpui-kit/crates/shell/src/style.rs`, `vendor/gpui-kit/crates/shell/src/lib.rs`, `vendor/gpui-kit/crates/shell/src/materialize/components/editor.rs`, `defaults/frame/settings/main.js` (`scaleStep`), `defaults/frame/whatsnew/release-notes.js`, `defaults/mods/tabs/main.js`, `defaults/mods/picker/main.js`, `defaults/mods/win98/kit.js`, `defaults/mods/win98/taskbar.js`, `defaults/AGENTS.md`, `defaults/herdr.d.ts`, `scripts/settings.test.ts`, `src/mods.rs` (`settings`), `src/mods/render_tests.rs` (`tabs_leave_room_for_macos_traffic_lights`, `interface_zoom_and_text_size_rows_show_their_percent`), `src/window.rs` test `mod_ui_scales` (a mod's px and text through the bridge, live from settings.json)
- Upstream: none yet

### Focus follows mouse

`feat/focus-follows-mouse` (branched from `master`, fork only): `focus_follows_mouse` in settings.json (default off) focuses a terminal pane, a tab's or a layout's terminal block, once the pointer rests on it for `focus_follows_mouse_delay_ms` (0 to 2000, default 150; clamped when read), through `pane.focus` as a click would. The delay is a debounce: it runs from after the pointer rests, every real move starts it over, so sweeping across a pane on the way somewhere else doesn't focus it; at 0 ms the first move onto a new pane focuses it at once. Only a real pointer move counts: content scrolling or panes moving under a still pointer, and moves under 1 px (jitter, which neither starts nor delays a wait), keep the focus. After any key down, or a focus change made some other way (menu-bound keys, clicks, mods), a wait may only start once the pointer has gone more than 4 logical px from where it was then, so a pane the keyboard focused keeps the focus while the hand rests on the mouse. It never moves while a button is held (selections, pane and divider drags, link clicks), while a dialog, popup, peek or menu is open (toasts and window overlays don't count), while Herdr isn't the active window, or onto the pane already focused; a press, the pointer leaving the pane or the window, or any of those arriving cancel the wait. `focus.toggle_follows_mouse` flips the setting in settings.json and says so in a toast. Keys pressed while it is off hold the pointer too, so a key that turns it on keeps the focus where it is until the pointer moves 4 px. `write_person` returns `PersonSaved` (saved, broken or write failed); when it isn't saved the toast is a warning, "Focus follows mouse: on (not saved: settings.json has errors)" or "… (not saved: cannot write settings.json)", and a control-socket caller gets that as an error (the zoom keys apply unsaved). The rules are a pure `Follow` state machine (`still`, `moved`, `keys`, `fire`, `cancel`) the window feeds from its root's mouse-move, mouse-exit, mouse-down and key-down listeners; the pane under the pointer is the shown terminal block (the active tab's, zoom applied, or the layout's) whose painted bounds hold the pointer, unless a floating block or a window overlay is over it there, checked again against the pointer's position when the wait ends (hidden blocks keep stale bounds and terminal hover, so neither is trusted alone).

- Settings window: a "Focus follows mouse" toggle under Look > Mouse, and a "Focus delay" stepper (50 ms steps) shown while it's on. Off and 150 ms leave their keys out of settings.json.
- Files: `src/window/follow_mouse.rs` (`Follow`, `Gate`, `Step`, the window's `follow_mouse`, `follow_mouse_fire`, `toggle_follows_mouse`, `terminal_pane_at`, tests), `src/window.rs` (fields, root listeners, `focus.toggle_follows_mouse`, `write_person`/`change_person`, tests `follow_mouse`, `follow_mouse_shown` and `follow_beside_window`), `src/settings.rs` (`Person`, `focus_follows_mouse_delay`, tests), `src/commands.rs`, `defaults/frame/settings/main.js`, `defaults/frame/palette/main.js`, `defaults/herdr.d.ts`, `defaults/AGENTS.md`, `docs/commands.md`
- Upstream: none yet

### Settings window keeps a broken settings.json

`fix/feature-review` (branched from `master`, fork only): the Settings window's `change` reads settings.json, edits it and writes it back only if nobody changed it meanwhile (`writeTextFile`'s `expected`), up to three times. While settings.json doesn't parse (`broken`, set by `load`), it writes nothing and an error toast says "settings.json has errors: fix it first", so one toggle can't replace the file with `{}` plus that one setting. When all three writes fail, an error toast says so with the last error. settings.json that parses to something other than a plain object (null, an array, a number) counts as broken too, and when reading it or applying the edit throws, an error toast titled "Cannot read settings.json" gives the reason.

- Files: `defaults/frame/settings/main.js` (`load`, `change`, `save`), `scripts/settings.test.ts`
- Upstream: none yet
