# Herdr app

<!-- Written by the Herdr app on every start. Edits here are overwritten. -->

This folder is **the Herdr app's whole interface, as files**. The Herdr app
is a desktop app for terminals and coding agents, and it redraws itself from
these files while it runs: change a layout, a key or a panel here and the
window follows within a second. Shipped layouts and mods are Herdr's:
change settings for look, or clone before editing structure/code (see Layouts).
When someone asks for something ("a sidebar
of my PRs", "give ctrl+1 back to the shell", "make it warmer", "move the
agents list to the right"), you do it by editing these files or writing a
mod. Nothing in the default interface is special: the spaces sidebar, the
agents list, the tab bar, menus, dialogs and notifications are built
from the same parts you have.

## Two programs

- **`herdr-app`**, the window: spaces, tabs, layouts, panels.
- **herdr-core (`hc`)**, which runs every terminal and agent. They keep
  running when the window closes or updates. You can use `hc` yourself:
  start a terminal, read its screen, send it text, wait for an agent
  (`hc --help`). The Herdr app is one client of it.

`herdr-app` controls this app; `hc` controls herdr-core.
`herdr-app --version` prints the version, and on this fork's builds the
commit beside it (`herdr-app 0.1.13 (fork c7a05ef)`).

## What you can and can't do

You can: rearrange and restyle the whole window (layouts), rebind any key,
write panels (mods) that list, filter and act on spaces, tabs, agents
and terminals, open dialogs, popups and floating blocks, show a live
terminal inside a block, run any app command, run programs (`exec`) and
fetch URLs, keep state per mod or shared between mods, animate natively.

You can't: use HTML, CSS, a web view, Node APIs or `setTimeout`; draw
outside your block except through overlays; change how a terminal itself
draws text; read or change other mods' code at run time.

## What mods are written in

Mods use **gpui-kit** (its gpui-shell): JavaScript that describes **GPUI**
elements, a native GPU UI (`div()`, flex, children,
`.on_click`). There's no DOM: the script describes, Rust draws. Background:
https://gpui-kit.com. The Herdr app carries its own copy with additions
(`frames`, `loop`), so **the type files here are the truth**:
`gpui-kit.d.ts` for elements and `herdr.d.ts` for the app's data and
commands. They're rewritten on every start to match this version.

## Changing things safely

- Edits apply at once. If a file is invalid, the app keeps the last good
  version and writes why to `<name>.error` (`layout.error`, `keys.error`,
  `settings.error`, `programs.error`). Check after every edit.
- `herdr-app check <mod folder>` loads a mod once and checks its initial
  description and eager native elements, including virtual lists and motion.
  It does not lay out the mod or run deferred rows, nested renders, or later
  async states. Never call a mod done without a passing check; also try it
  in the app.
- **Escape hatch:** `ctrl-alt-shift-escape` always works: it sets the shown
  layout and `keys.json` aside (`*.reset-<time>`) and goes back to the
  default `main` layout. Nothing can rebind it.

## Seeing the window

`herdr-app shot [file.png]` saves the last rendered window frame as a PNG
and prints its absolute path (default: `herdr-shot.png` in your current
folder). Use it when debugging mod UIs: inspect spacing, colors, terminal
images, overlays, popups and floats. It captures the app, not the desktop,
at the window’s physical pixel size, even when covered or minimized. No
screen-recording permission is needed. Herdr must be running; before its
first frame or during GPU recovery it reports an error instead of a PNG.
`HERDR_APP_HOME` selects which running window to ask, as with `status`.

## The files

| File | What it decides | Applied |
|---|---|---|
| `layouts/<name>.json` | the window and its whole look: blocks, theme, fonts, backgrounds, pane frames; one file per layout (`main`, `terminals`, `win98`, any you add) | live |
| `keys.json` | what every key does | live |
| `settings.json` | the person: zoom, sounds, fonts for every layout | live |
| `mods/<name>/` | panels: small JavaScript programs | live (hot reload) |
| `programs.json` | how programs show (name, icon, tile), on top of the built-in table | live |
| `icons/` | the shipped program icons | written by Herdr (put your own elsewhere) |
| `frame/` | the app's own screens (welcome, ...) | rewritten by Herdr on every start: don't edit, it won't stick |
| `view.json` | this screen's selected space/tab/pane, zoom, settled tabs, recent spaces and seen attention | written by Herdr |
| `shared.json`, `data/`, `seen.json`, `installed.json`, `window.json`, `windows.json` | mod state, bookkeeping, where the window was (size, maximized), mods' windows (spots, kept ones) | written by Herdr |

Shared state is saved atomically in the background, with the latest change winning.
If `shared.json` is invalid, Herdr logs a warning and keeps it as
`shared.json.corrupt-<time>` before starting empty.
`herdr-app check` uses empty shared state in memory and leaves this file untouched.

## The model: spaces, tabs, terminals

Every layout shows the same world; a layout may show less, never a
different one. Each machine keeps its spaces, tabs and splits in its core's
`herdr-app` document. Connected screens share that layout and live terminals,
not their focus, zoom or attention. Use commands to change structure; there
is no `spaces.json`. On simultaneous edits the newest accepted document wins
(no merging); structural commands fail while that machine's layout is unavailable.
Mod pane props are stored as given, never machine-qualified. Space order is local
machine first, then machines by name; `space.move` stays inside its machine's block.
In the expanded Spaces sidebar, drag a space to reorder it within its machine;
the highlighted line shows where it will land.
In the tab bar, drag a tab to reorder it: it follows the pointer and a line shows where it will land (either end and the gaps count); Esc or leaving the bar cancels. A tab's right-click menu has Move left and Move right, and Move to space… when its machine has another space: the shipped `tab-move` picker (`mod.open {name: "tab-move", props: {id?}}`, also in the palette as Move tab to space…) runs `tab.move_to_space`. `tab.move_left` / `tab.move_right` move the shown tab by keys (Cmd+Shift+Left / Right on macOS; elsewhere unbound by default, so Alt+Shift+[ / ] stay with terminal programs: bind them in keys.json).

- A **space** is a place to work, on one machine: this one, or another
  from `hc machine ls` (`spaces()[i].machine`). Herdr leaves out machines
  added with `--temp` (an agent's short-lived sandbox): no spaces, not in
  `machines()`, and mods listing machines skip `temporary`. Names work as in Herdr:
  a space nobody named is called after where its first terminal is now
  (its repo, else its folder, `~` at home) and follows it; once named
  (`space.rename`), the name stays; renaming to "" goes back.
  `spaces()[i].named` says which. Where a space is now is its terminals'
  live folders (`panes()[i].cwd`), not the folder it was made in: new
  tabs start where its focused pane is, and the default mods take a
  space's repo from its first terminal.
  If starting in an inherited folder fails, new spaces, tabs and splits retry in
  that machine's home. Explicit invalid folders still fail.
- A **tab** is a piece of work in a space: its terminals and how they're
  split right and down. One nobody named is called by its place (1, 2,
  ...), as in Herdr; `tab.rename` names it ("fix login"). A space
  has one or more; closing the last one closes the space (it stays in
  `recent`).
- A tab can be **settled** (`tab.settle`): put away on this screen, still running;
  `settled` on each tab. The shipped layouts don't use it (they list every
  tab); a mod that does can hide or fold settled tabs. Showing one, or an
  agent in it needing the user, brings it back.
- A **pane** is a terminal on screen, or a **mod** beside them (a markdown
  viewer, a board, a git panel): `pane.split {direction, mod, props}`. One
  nobody named shows its `label` (below); `pane.rename` names it
  ("server"), saved with its tab, and
  `panes()[i].name` gives the name (null: none). With `pane_auto_names` on, a pane at a bare shell nobody named shows two words of its own instead (`panes()[i].auto_name`, "hazy-inlet"; see The person).
  What a pane or agent is called is one value, `label`, on `panes()`,
  `agents()` and `herdr-app ls`: the first of the pane's name (one with a
  letter or digit), its `auto_name`, its title when telling, the agent
  session's own name (`session`), the program's name, the terminal's
  address. A title is telling when it says something of its own:
  `✓ build passed` is; a path, a `user@host`, only digits, the program's
  name or command line, or a throwaway (anything holding a UUID, or
  universally unique identifier, `tmp.…` files, an editor's
  `claude-prompt-<id>.md - Nvim`) is not. `session` is read from the program's
  session file (`session_name` in Programs; Claude Code's `/rename`) for
  the process running in the terminal on this computer; null otherwise.
  Name things by `label` rather than re-deriving it from `title`, which
  stays the title as the program set it (spinners cut).
  A
  mod pane splits, zooms, closes and gets a header like any pane, keeps
  the keyboard when clicked, and is saved with its tab; its `props` tell
  it what to show (a file, a folder). Closing a terminal pane ends its
  terminal, and a program exiting closes its pane, even if Herdr was closed.
  Terminal panes are kept by their machine's core (`keep: true`). A lost
  terminal (core crash, reboot, logout) gets a shell in its last folder,
  with the same address; its original command is never run again. Splits
  and mod panes stay. An exited or killed terminal is closed, not restarted.
  Focus, zoom, recent spaces, settled tabs and seen attention stay on this screen.
- Every terminal is a core terminal. Herdr labels the ones it starts with
  `herdr.space=<id>` and `herdr.tab=<id>`. **A terminal with those
  labels shows up there**, even one you start yourself:
  `hc run --keep --label herdr.space=<id> --label herdr.tab=<id> -- claude`.
  Labels use ids local to that machine; remote ids shown by the app have a
  `<machine>/` prefix (strip it when writing labels on that machine). Adoption
  also enables core keep; `terminal.park` removes the pane and disables keep.
  When a core change is needed, `terminal.keep` / `terminal.park` acknowledge
  with `pending: true`, then apply after the machine replies; failures show a toast.
  Normal quit waits for pending terminal changes and layout writes; remote waits are bounded.
  An id Herdr doesn't know becomes a new space or tab (unnamed: Herdr's names).
  Terminals without the labels (another app's) stay out of Herdr's
  screens, and so do other machines' terminals, unless Herdr started them
  for a space on that machine.
- **Agents** are terminals whose program reports a status (OSC 7501):
  `working`, `blocked`, `idle`, or `done` = finished while the user wasn't
  looking (it becomes `idle` once they look). A space's and a tab's
  status is the most urgent of its agents: blocked > done > working > idle.
  A terminal with no 7501 report whose foreground process is a known agent
  (Claude Code, Codex, OpenCode, Pi, Hermes and others: the manifests in
  `defaults/harnesses`, plus your own in `harnesses/*.toml` in the home
  folder, read at start) is an agent too, with `app` set to the manifest's
  `id`. Its status is read off the bottom of its screen, or else its window
  title, when either changes (checked about twice a second): needs-input
  prompts are `blocked`, a turn in progress `working`, the empty prompt box
  `idle`. It starts `idle` and keeps its last status while no rule matches,
  except that `working` with no screen change for 30s becomes `idle`; an
  `idle` after `working` waits about a second to be sure. A turn it ends
  while you aren't looking shows `done` until you look; that is kept in
  the app, not shared with other clients through `herdr.seen`, and isn't
  remembered across a restart. This covers
  a backgrounded Claude Code session resumed into a pane, which no longer
  reports. A 7501 report always wins; the entry goes when the process in
  front stops being the agent or the terminal ends.
- **Sub-agents** are terminals an agent started with `hc run`: the core
  records the starter as `parent` (see `terminals()`). They belong to their
  parent's tab without a pane; the `agents` mod shows them (a parent's
  row counts its live ones, a copy icon and 2, in the colour of the most urgent:
  blocked, working, else dim; `agents_subagents` folds or hides them), `terminal.peek` opens one,
  `terminal.keep` gives one a pane. A program's OSC 7501 child records
  (reports with an `id`, in `terminals()[i].records` by id) are sub-agents
  too, with no terminal of their own: a row each under the terminal,
  named by the record's `title`, else its id. Claude Code reports its own
  sub-agents (the Task tool) this way: one record each, `id=<agentId>`,
  `app=claude-code`, `title=<description>`.
  Ctrl-click (Cmd-click on macOS) a live terminal address in terminal text to
  peek at it. Bare addresses refer to the machine of the terminal showing them;
  use `machine/address` for another machine. Names and non-live addresses aren't links.
- **Agent navigation order** belongs to the mods. A mod displaying agents calls
  `setAgentOrder(list.map(a => a.address))` on render so `agent.next` /
  `agent.prev` follow its list.

## Layouts: `layouts/<name>.json`

Each file is one whole window. `layout.switch` (`ctrl-shift-l`) cycles
through them, `layout.switch {name}` picks one; `shared("layout")` is the
shown one's name. Herdr ships `main` ("Herdr": spaces by machine and the
agents list on the left, `workspace-chrome`; tabs as pills with a + menu
that starts a terminal or any installed agent, `new-menu`; panes as cards
with a header; Files and Changes panels on the right, `files` and
`changes`, opened from the tab bar and remembered per space (which ones:
shared `"workspace.panels"`, set by the welcome); a status bar,
`workspace-status`), and as examples `terminals` ("Terminals": tabs in
the title bar, panes as cards, a space picker) and `win98`
("Herdr 98": a whole desktop in one mod, `win98`). Add a layout by
writing a new file. The Files panel supports Windows folder paths with spaces,
parentheses and apostrophes, and preserves Unicode file and folder names
(including Chinese). Directory links expand as folders; their contents load
only when expanded.

**Herdr's own layouts and mods are read-only.** They are replaced whole
on every start. Use settings.json for look (fonts, sizes, theme, colors,
sounds), where the layout leaves them unset. Herdr (main) has no look of
its own, so your settings decide it; Herdr 98 and Terminals bring their own
look. The theme picker overrides any layout. To change a shipped layout's
look beyond that, or its structure/code, clone it and edit the clone:

```sh
herdr-app do layout.clone '{"name":"main","as":"my-layout"}'
herdr-app do mod.clone '{"name":"files","as":"my-files"}'
```

The default clone name is `<name>-copy`; an existing name is refused.
`layout.clone` switches to the clone. In the palette: Duplicate layout
or Duplicate mod. Clones are yours and never updated. They record
`cloned_from: {name, version}` and copy the layout and its clicks as-is,
including its look. A cloned layout keeps its mod references:
clone a mod too and point your layout's `content.mod` to it to change code.
Mod clones rewrite their own `mods/<name>/` asset paths and self-opening
overlay names to the new name.

If you edit a shipped file anyway, the next start moves it to
`<name>-edited` (a mod folder or layout JSON; a number is added if taken),
copies its layout clicks, restores Herdr's version and shows a toast.
No-longer-shipped things go if
untouched, otherwise they stay.

Clicks (sizes, shown/hidden, stack child, floating spots, theme picks,
background, text size, pane header height and border/focus look, and
spacing) live in `layouts/.state/` and survive restarts. Don't edit those files.
A deliberate layout-file edit beats a click on that field.
Spacing clicks come from `layout.spacing` (the Settings window's Layout page,
under Spacing): the tab block's `pane.gap`, the `padding` and `gap` of the
row or column holding the tab block, and the root's `terminal_padding`.
They work on shipped and personal layouts alike and never change the file.
The tab block is the one on screen (not a hidden one or a stack's other
child). Spacing clicks are 0 to 40 px; a file value above 40 shows as is, and
the first step moves it to 40. An area click stays on its node if the file
later moves the tab block elsewhere; `layout.reset` clears it.
`layout.spacing` fails while the layout file has errors. Mods read the
result with `layoutSpacing()`, which redraws only when spacing changes.
`layout.reset {name}` resets a shipped layout and its clicks, keeping its
previous file aside as `<name>.json.old-<time>`.

A layout is a tree of nodes. Four types:

- `block`: shows content. `"content"` is `{ "mod": "<folder>", "props": {...} }`,
  `{ "terminal": "t7" }`, `{ "tab": "active" }` (the shown tab's
  terminals as it arranges them), or `{ "agent": "focused" }` (one agent's
  terminal, full size: the one chosen with `agent.focus` / `agent.next`, else
  the focused pane's agent, else the first agent).
- `row` / `column`: children side by side / stacked.
- `stack`: shows one child (`"active": "<child id>"`), with an optional
  `"header"` (a mod that draws tabs, or `"tabs"` for a plain built-in strip).

Properties any node can have:
- `id` (unique; commands use it), `size`: `{ "px": 240 }` or `{ "weight": 2 }`
  (shares of what fixed siblings leave), `min` / `max` (px), `hidden`.
- Paint: `background` (a color; `{ "gradient": ["#2a6cbd", "#0b2a57"], "angle": 165 }`;
  or `{ "image": "backgrounds/sky.jpg", "fit": "cover" | "contain" }`, a path in
  the home folder), `border` (`{ "width": 1, "color": "border" }`), `radius`,
  `shadow` (`"sm" | "md" | "lg"`), `focus` (`{ "border": {...} }` when the
  focused pane is inside).
- `padding`: space inside the node's edge, px: `16` or `[vertical, horizontal]`.
- row/column: `gap` (px between children), `divider` (`{ "width": 1, "color": "border" }`),
  `resizable` (default true: drag the boundary). This applies to that row/column only.
- `movable` (default true): panes can be dragged by the app's top-center grip within a split tab.
  Unlike `resizable`, it inherits: the nearest explicit ancestor wins, through the
  tab tree and then the layout's tab block and its ancestors. Root `"movable": false`
  opts the layout out. Composition terminal blocks aren't tab panes and never drag.
  Center drop swaps panes; an edge drop places beside it. Escape or dropping outside
  another pane cancels. Zoomed tabs and one-pane tabs don't drag. The grip appears
  on top-edge hover, regardless of the header's position or presence. Only the grip
  starts dragging; header mods don't need drag handlers. `panes()` exposes `movable`.
- `floating`: blocks over this node (below).
- `window_drag: true`: dragging the node moves the window, a double-click
  maximizes it. A mod's own control inside opts out by stopping the press:
  `.on_mouse_down("left", (_e, cx) => cx.stop_propagation())`.
- Root only: `"titlebar": "none"` hides the system title bar so the layout
  draws its own (a bar mod in a `window_drag` block). macOS keeps its window
  buttons over the layout at `"traffic_lights": [x, y]` (px); leave them
  room. Where the system draws no buttons (`window().buttons`, Linux and
  Windows), the bar draws them with `window.minimize`, `window.maximize`, `window.close`.
  Windows' shipped tab bar and system strip use vector window icons, with a
  restore icon when maximized, rather than font glyphs.
- Root only: `"preview": "layouts/<name>.svg"` (or png): the picture the
  settings screen shows for this layout. Draw one when you make a layout;
  without it the person sees a plain frame. New layouts and `themes/*.json`
  show up in settings by themselves.
- Root only: `"terminal_menu": "my-menu"` chooses the mod for terminal
  right-clicks (including Shift+right-click). Omit it for the shipped `menu`;
  an unknown mod logs a warning and falls back to `menu`. Clone a shipped
  layout before changing this field. Other menus are unchanged.
- Colors are theme names (`border`, `accent`, `surface`, ...) or `#rrggbb[aa]`.

A personal terminal menu replaces the whole menu; it doesn't merge items.
To add an item to the shipped menu instead, give a mod `"menu_items"` (see
Mods); a personal menu reads them with `menuItems()` and shows what it likes.
Create `mods/my-menu/main.js` and `mod.json` (e.g.
`{"place":"none","overlay":{"width":280,"height":100}}`), then set the layout root's
`"terminal_menu": "my-menu"`. It opens as a pointer popup, default 220 px wide and at most 420 px tall (it fits its items);
its `mod.json` overlay width/height override those defaults. It gets
`props()` = `{tab: "pane", target: {pane, terminal, selected, cwd, last_output}, height, overlay}`: `height` is the room inside the popup in px (its most, or what fits in the window, less its border), to scroll a long list in (the shipped menu scrolls its items and keeps Close pane pinned below them); `selected` is whether the terminal had a text selection when right-clicked (the right-click leaves it in place), `cwd` its folder or null, `last_output` whether `terminal.copy_last_output` has a finished command's output to copy. Always use
`target.terminal` or `target.pane` in actions, not the focused terminal:
right-clicking a different terminal must act on the one clicked. A terminal
shown outside a tab (a layout terminal block, a window) has no pane:
`target.pane` is its block's id, not in `panes()`, so leave out pane items
there, as the shipped menu does. For example:

```js
import { View, div } from "gpui-kit";
import { props, run, closeOverlay } from "herdr";
export default class MyMenu extends View {
  render() {
    return div().id("split-target").child("Split this pane").on_click(() => {
      const {target, overlay} = props();
      closeOverlay(overlay);
      run("pane.split", {pane: target.pane, direction: "right"});
    });
  }
}
```

### The look: on the layout, cascading

A layout's own look wins; settings.json fills what it leaves unset.
The theme picker overrides both. These go
on any node, and the nearest node up the tree wins, so the root sets the
layout's look and a block can differ:

```json
{ "id": "root", "type": "row", "theme": "graphite", "colors": { "accent": "#a7b4c6" },
  "font": "Geist Mono", "font_size": 15, "ui_font": "Geist",
  "terminal_padding": [10, 6], "terminal_opacity": 1, ... }
```

- `theme`: `graphite`, `ink`, `moss` (dark), `paper` (light), or the name
  of a file in `themes/`; or one for each appearance:
  `"theme": { "light": "paper", "dark": "graphite" }` follows the system's
  light or dark mode (or the person's `appearance`). `app.theme {name}`
  sets the half of the name's lightness. A layout without a theme shows
  settings.json's theme, else graphite or paper. `colors` overrides any theme color by name (see
  `theme()` below), for terminals and panels alike: overrides apply in
  both appearances, so name theme colors (`chrome`, `canvas`) rather than
  writing hex when a layout should work light and dark.
- `themes/<name>.json`: `{ "background", "foreground", "selection",
  "cursor"?, "ansi": [16 colors], "colors"?: {...} }`, all `#rrggbb`; panel
  colors are derived from these, `colors` overrides them.
  With `panel_colors` at its default, each derived panel color is the
  background stepped toward the theme's own `foreground`, so panels carry
  the theme's tint: `canvas` a hair darker than the terminal, then
  `chrome` (sidebars, bars), `popover`, `raised`, `border` and `input`
  further on; `hover` and `selected` are the `foreground` at low alpha.
  `ring` (focus rings) and `focus` (the active pane's border in the
  shipped Herdr layout) are the accent in use when it reads at 3:1 on the
  background, else a stand-in that does; `colors.focus` sets `focus` apart
  from the accent. Toasts have their own colors, held to contrast floors
  on every theme: `toast`, `toast_border`, `toast_shadow`,
  `toast_foreground`, `toast_muted`, and `toast_info`, `toast_success`,
  `toast_warning` and `toast_error` (the level stripe); set any in
  `colors` to replace it. A `colors` value that isn't `#rrggbb` or
  `#rrggbbaa` is reported as a problem. A theme file or layout look whose
  own `colors` paint its panels (`surface`, `popover` or `raised`: Win98,
  Terminals), or whose `foreground` sits under 3:1 on its background or
  on the wrong side of it,
  derives what it leaves unset the classic way (`panel_colors`).
  Herdr ships
  `atom-one-dark`. The original unedited Atom One Dark and Win98 theme files
  are upgraded to readable terminal colors; edited theme files are kept.
  A theme named like a layout (`themes/win98.json`) is
  that layout's own: Settings offers it only there.
  `herdr-app rex-themes` writes `themes/rex-<name>.json` for each theme of the installed Rex app (read from the app on every run, never shipped), replacing only `rex-*` files; `--dry-run` lists what it would write.
- `font` / `font_size` / `letter_spacing`: the terminals' (px at zoom 1); `ui_font`: the mods'
  (their text sizes are their own, in px, and follow zoom, `ui_zoom` and `ui_text_scale`). Optional: without
  them a layout uses settings.json's, else Herdr's (JetBrains Mono 14, Geist).
  Herdr (main) leaves these unset; Terminals sets text size 15, and Herdr 98
  brings its own fonts.
  JetBrains Mono, Geist Mono and Geist ship with Herdr; any installed font
  works (a terminal font the machine lacks falls back to JetBrains Mono).
  A layout brings its own fonts: `.ttf`/`.otf` files in a mod's `fonts/`
  folder (or the home's `fonts/`) load at start and on the next look change,
  so a layout and its mod folder travel together (Herdr 98's do: its pixel
  fonts are in `mods/win98/fonts/`).
- `line_height` (0.8..2): the terminals' row height, as a multiple of the
  font's height. Default 1.15; 1 packs rows like most terminals do.
- `font_weight` (100..900): the terminals' text weight. Default 500
  (Medium); 400 is Regular, 300 Light, using the family's nearest face.
  Bold text uses 700, or `font_weight` when that is heavier.
- `font_weight_bold` (100..900): the weight bold terminal text draws at.
  Unset, 700 or `font_weight` when that is heavier. Set it to keep bold
  close to a light `font_weight` (250 with bold 550, say).
- `pane_scrollbars` in settings.json (default `true`): show terminal scrollback
  with a slim thumb and track. Drag the thumb or click the track to jump there.
  With little or no terminal padding, the hit lane overlays the last 8px;
  terminal columns do not change. Hidden in alternate-screen programs.
- `terminal_opacity` (0..1): below 1, the background behind a terminal
  (a pane frame's, the layout's gradient) shows through its text.
- Switching layouts switches the look. The theme menu changes the shown
  layout's root `theme`.
- Today only the root's look applies; a block's own look is coming.

### Pane frames

The tab block says how each pane of the shown tab is wrapped:

```json
{ "tab": "active",
  "pane": { "gap": 6, "radius": 6, "background": "surface",
            "border": { "width": 1, "color": "border" }, "focus": { "width": 1, "color": "focus" },
            "header": { "content": { "mod": "pane" }, "height": 30, "position": "top", "reveal": "always" },
            "lone": false } }
```

- The look applies to every pane; `focus` replaces the border while the
  pane has the keyboard; `gap` is the space between panes.
- Each pane's content sits in by the wider of `border` and `focus`, so moving the focus never resizes a terminal; with the narrower line drawn, the rest shows the frame's `background` if it sets one. settings.json's `focus_border_width` replaces the `focus` width.
- `header`: a mod drawn on each pane, one copy per pane, with
  `props().pane` and `props().terminal`. It reads its live facts from
  `panes()` (name, auto_name, label, session, title, folder, branch, agent status, focused, zoomed, text_size) and its
  buttons run pane commands with that pane (`pane.split`, `pane.zoom`,
  `pane.close`). `position`: `top` or `bottom`; `reveal: "hover"` shows it
  only while the pointer is on the pane, drawn over the terminal so the
  terminal never resizes. The shipped `pane` mod is an example header.
- `lone: true` frames a tab's only pane too (default: only when split).
- Each layout has its own, so one can stay bare while another makes
  every pane a card.

### Floating blocks

```json
"floating": [ { "id": "board", "content": { "mod": "board" }, "edge": "left", "align": "center", "peek": 52 } ]
```

A floating block sits over its node, `edge` (`top`, `right`, `bottom`,
`left`) and `align` along it (`start`, `center`, `end`), 12 px in. It sizes
to its content (render nothing to hide it). People drag it anywhere; on
release it springs to the nearest edge and third, and Herdr saves the new
spot. Clicks and hovers on it never reach what's underneath.

`peek` (side edges only) makes it a drawer: only that many px next to the
edge show until it's hovered, then the rest slides out; it slides back a
moment after the pointer leaves. The mod draws one full layout and never
changes it on hover (that's what keeps it smooth): put what should peek
(status, icons) on the edge side of each row.

Herdr draws the frame around a floating mod (theme background, border,
rounded corners, shadow; accent border while dragged), like a dialog's,
with 5 px above and below. The mod draws only its content, no panel: on a
side, rows can run edge to edge. It reads `placement()`:
`{ edge, align, vertical, hovered, dragging }`. Lay out vertically when
`vertical` (left/right edges), horizontally otherwise; on the right edge,
mirror rows so their edge side is on the right.

The default window:

```json
{ "id": "root", "type": "row", "divider": { "width": 1, "color": "border" },
  "children": [
    { "id": "sidebar", "type": "column", "size": { "px": 240 }, "min": 40, "max": 480,
      "divider": { "width": 1, "color": "border" },
      "children": [
        { "id": "spaces", "type": "block", "content": { "mod": "spaces" } },
        { "id": "agents", "type": "block", "content": { "mod": "agents" } } ] },
    { "id": "work", "type": "column",
      "children": [
        { "id": "tabs", "type": "block", "size": { "px": 30 }, "content": { "mod": "tabs" } },
        { "id": "main", "type": "block", "content": { "tab": "active",
          "pane": { "gap": 6, "radius": 6, "border": { "width": 1, "color": "border" },
                    "focus": { "width": 1, "color": "focus" } } } } ] } ] }
```

Sidebar on the right: swap the two children of `root`. No sidebar: set
`"hidden": true` on `sidebar` (the user can toggle it: `ctrl-shift-b`).
Narrower than 160px, the sidebar mods draw a compact rail; folding resizes
`sidebar` to 44 (76 on macOS, room for the traffic lights). On macOS,
the tabs also leave room for the window buttons when the sidebar is narrower.

### Layouts and mods other people made

People share layouts at herdr.dev/layouts and mods at herdr.dev/mods. Each
is a folder laid out like this home: a layout has `layouts/<name>.json` and
the `mods/<name>/` it needs; a mod is a piece for the person's own window,
so it has only `mods/<name>/` and maybe a `keys.json` with the keys it
expects. Both have a `RECIPE.md` (the brief to build it from scratch) and a
`preview.png`. The files are one build of the recipe. Asked to add one
(`herdr.dev/layouts/<owner>/<name>`, `herdr.dev/mods/<owner>/<name>`, or a
folder on GitHub):

1. Read the recipe and every file before writing anything. Before adding,
   tell the person if a mod reaches the network, runs programs (`exec`,
   `hc`) or reads files outside its own folder.
2. Copy the layout file to `layouts/` and each mod folder to `mods/`. Leave
   the recipe and preview out. Never overwrite: if a name is taken (yours
   or Herdr's), rename the new one: its file or folder, the `mod` names in
   the layout, and the mod's own `mods/<name>/` paths.
3. Fit it to this setup only where its assumptions don't hold (a missing
   program, a name clash) or the person asks. If the files don't work on
   this version, or the person wants it different, build it from the
   recipe instead.
4. A mod's `keys.json`: merge it into the person's. Add its sections and
   keys, and ask before replacing a key they already use.
5. `herdr-app check` each mod; for a layout, then `layout.switch {name}`.
   The person's own layout stays as it was; `ctrl-shift-l` switches back,
   and deleting the files removes it.

To share one: put that folder in a public GitHub repo (one folder each if
there are several), write its `RECIPE.md`, save a screenshot of it as
`preview.png` and add the repo topic `herdr-layout` for a layout or
`herdr-mod` for a mod. It shows up at herdr.dev/layouts or herdr.dev/mods
within the hour.

A `RECIPE.md` is a brief: another agent with an empty folder builds the
layout or mod from it. Write it from the real files, in this order:

1. `# Name`, one paragraph of what the person sees, `![Name](preview.png)`.
2. **What you see:** the regions top to bottom or left to right, their
   sizes, colors and type; what each one shows.
3. **How it behaves:** every agent status (working, blocked, done, idle)
   and how it looks; what clicks, keys and hovers do; what moves.
4. **Built from:** the layout's node tree and blocks, each mod and its
   `props` roles, the shipped mods it uses, the `herdr` functions and
   commands it calls, any programs it runs, its art.
5. **Watch out:** what tripped the build up and what a fresh build would
   get wrong; limits (sizes, counts); controls that do nothing yet; what
   it assumes about the setup.
6. **Done when:** checks that prove a build works, each one something the
   build really does.

The person and what they see come first; code only in Built from and
Watch out. Exact over vague (`#ab7748`, 29px, `agent.focus`), and nothing
the build doesn't do.

## Keys: `keys.json`

On upgrade, an untouched keys file from an earlier release gets the current
shortcuts. Any edit, even whitespace, keeps the entire file as yours; new default
shortcuts must then be added by hand.

```json
{
  "global":   { "ctrl-shift-t": "tab.new", "ctrl-shift-o": "pane.split:right", "alt-1": "space.select:1" },
  "terminal": { "ctrl-shift-c": "terminal.copy", "ctrl-left": "send:\u001bb", "ctrl-1": "terminal" },
  "layout:terminals": { "ctrl-shift-pagedown": "space.next", "ctrl-shift-pageup": "space.prev" },
  "mod:editor": { "ctrl-s": "editor.save" }
}
```

- Sections: `global`, `layout:<name>` (while `layouts/<name>.json` shows),
  `terminal` (a terminal has focus), `mod:<name>`. The more specific section
  wins: `layout:terminals` over `global`, `terminal` over both.
- An action is a command (`name`, or `name:value`: the value fills the
  command's first argument), `send:<text>` (types into the terminal),
  `<mod>.<command>` (delivered to that mod's `nextCommand()`), or `terminal`
  (the program gets the key).
- Tab and Shift-Tab move focus between a mod's focusable elements and never reach its `on_key_down`. A mod that wants them binds them in its `mod:<name>` section to `<mod>.<command>` and reads the command with `nextCommand()`:
  ```js
  // keys.json: "mod:chooser": { "tab": "chooser.next", "shift-tab": "chooser.prev" }
  cx.spawn(async (cx) => { for (;;) { const c = await nextCommand(); if (c === "next") this.move(1); else if (c === "prev") this.move(-1); cx.notify(); } });
  ```
- Named, typed command arguments use an object instead of a string:
  `"ctrl-shift-r": {"command": "layout.resize", "args": {"id": "sidebar", "px": 240}}`.
  `command` names an app command; `args` is an object (optional, defaults to `{}`).
  It can pass several arguments, numbers, booleans, arrays or nested objects.
  String actions keep their literal first-argument shorthand, including numeric
  indexes: `space.new:{work}` names a space `{work}`, and `space.new:{}` names it `{}`.
  Invalid binding objects or rejected arguments show an error toast when pressed;
  the key does not go to the terminal program.
- Plain ctrl+letters go to the program unless bound. Programs rely on
  ctrl+c, ctrl+d, ctrl+l, ctrl+r, ctrl+w, ctrl+a, ctrl+e, ctrl+u, ctrl+k,
  ctrl+z and ctrl+[ (Escape); ctrl+alt is AltGr on Windows; option+digits
  and letters type characters on many Mac keyboards (`[`, `@`). Don't pick
  these on your own. If the person asks for one, bind it and tell them what
  it takes from typing.
- Windows AltGr and dead-key compositions type text in terminals. macOS
  dead keys compose accents through native text input; Option keeps its
  keyboard-layout characters rather than acting as Alt.
- On macOS, shifted punctuation shortcuts accept both spellings from the
  current layout: `cmd-shift-]` and `cmd-}` match the same US-layout keypress.
  Existing `keys.json` files don't need to change.
- Terminal keys honor the program's negotiated Kitty mode, including on Windows.
- Windows terminal Escape stays separate from the following key.
- Search: `ctrl-shift-f` (Cmd-F on macOS) opens a find bar inside the focused terminal;
  Enter/Shift-Enter moves through matches and Escape closes it while the find field
  has focus. Clicking the terminal puts typing, including Escape, back in the program;
  the bar and its matches stay.
  Each terminal keeps its own query, and reopening selects it so typing replaces it. Search covers the screen and scrollback without replacing text selection.

## Commands

The one way to change anything; keys, mods (`run(name, args)`), menus and
you use the same list. `herdr-app commands` prints every one with its arguments
(which are required) and what it does. Unknown argument names are errors and list
valid names; they never fall back to the current target. For example, closing a
space uses `herdr-app do space.close --id <id> --confirmed true`, not `--space`.
Target IDs (`id`, `pane`, `space`, `terminal`) must be strings. Other non-null
types are rejected before a current-target default applies; null means omitted.
The main ones:

- Spaces: `space.new {name?, folder?, tab?, argv?, task?, machine?}`
  (machine: another machine from `hc machine ls`; omitted: the focused space's
  machine for keys and clicks, this computer for scripts; `""`: this computer.
  The space lives there, its
  terminals start in that machine's home unless folder, and its tabs and
  splits stay there; `spaces()` gives each space's `machine`, null here),
  `space.close {id?, confirmed?}`, `space.rename {id?, name}` ("": Herdr's name again),
  `space.move {id, index}`, `space.select {index | id}`, `space.next`, `space.prev`.
  With no `argv`, the target core chooses that machine's default shell (PowerShell 7 when available on Windows).
- Tabs: `tab.new {name?, space?, folder?, argv?, task?}` (argv: an
  agent to run, task: its first message). Agents run in your interactive
  shell (including fish), which stays open after the agent exits. A terminal
  that exits with a nonzero code within two seconds of launch shows an error
  toast with its exit code and last output lines. Or `tab.new {name?, space?, mod,
  props?}` for a tab whose only pane is a mod (a file, a board; split a shell
  in beside it like any tab), `tab.close {id?, confirmed?}` (closing the last tab
  closes its space and asks first unless confirmed),
  `tab.rename {id?, name}` ("": its number again), `tab.move {id, index}`,
  `tab.move_left {id?}`, `tab.move_right {id?}` (one place; nothing at the end),
  `tab.move_to_space {space, id?, index?}` (the whole tab, the shown one by
  default, to another space on its machine, last unless index; shown there,
  its terminals keep running; a space it leaves empty closes),
  `tab.select {index | id}`, `tab.next`, `tab.prev`,
  `tab.settle {id?}`, `tab.unsettle {id?}` (index, next and prev
  skip settled tabs).
- Panes: `pane.split {direction: "right" | "down", pane?, argv? | mod?, props?}` (a new
  terminal, or a mod pane), `pane.close {pane?}`, `pane.rename {pane?, name}` ("": unnamed again),
  `pane.set_icon {pane?, icon}` and `tab.set_icon {id?, icon}` (`icon`: `{kind: "glyph" | "emoji", value}` or `{kind: "letter"}`; null or absent: the automatic icon again, so a misspelt key clears it; kept by id in the machine's layout and gone with the pane or tab; the shipped `menu` offers them as Change icon… and Clear icon, through the shipped `icon-picker`: Suggested, every Symbols Nerd Font glyph and every emoji, searchable by name and keyword. A mod keeping icons of its own opens the same picker with `openOverlay({mod: "icon-picker", kind: "dialog", width: 452, height: 392, props: {shared: "<key>", id, name, letter?, now?, extra?}})`: the pick is written to `shared(key)[id]` as `{kind, value?}` and Clear icon deletes it; `now` (`{kind, text}`) is the icon shown today, `extra` (`[{kind, text, word}]`) adds the mod's own kinds beside the letter),
  `pane.zoom {pane?}`, `pane.menu {pane?}` (the pane's menu at the pointer, the layout's `terminal_menu` as a terminal right-click opens it, with no selection: what a pane header's right-click runs; the shipped `menu` lists Copy with a selection, Copy all, Paste, Copy path when the folder is known, then Rename, Split right and down, Zoom, Change icon… and Clear icon, Float pane (or Dock pane and Bring to front), then where right-clicks go, Send back for a kept sub-agent, and Close pane), `pane.focus {pane | terminal}`, `pane.refocus` (the keyboard back to what has the focus after a mod took it for a moment, as a tab drag does: a focused mod pane gets its `focus` command again, else the focused pane or the agent shown), `pane.go {direction}`, `pane.swap {pane?, direction | target}`.
  Floating panes: `pane.float {pane?, rect?}` lifts a tiled pane above its tab's tiled panes, in front and focused (`rect`: `[x, y, w, h]`, fractions of the tab area; default centred at 60% × 60%); `pane.dock {pane?, target?, edge?}` puts it back: beside `target` (a tiled pane) on `edge` (`left`, `right`, `top`, `bottom`), else beside the pane it floated from on the same side at the same share, else beside the tiled pane focused last (an arranged tab places it there in reading order and arranges again, so a master pane comes back as master); `pane.float_toggle {pane?}` does either. `pane.float_move {pane?, direction? | rect? | to?, cells?}` moves one (by `cells`, default 2 columns or 1 row; or to `center`, `full`, a half `left`/`right`/`top`/`bottom` or a quarter `top-left`…; the shorthand `pane.float_move:center` works), `pane.float_resize {direction, cells?, pane?}` grows it right or down and shrinks it left or up, its top-left corner staying. `pane.raise {to?, pane?}` and `pane.lower {to?, pane?}` step it forward or back (`to: "front"` / `"back"`); focusing a float brings it to the front. `pane.float_cycle {reverse?}` focuses the shown tab's floats in turn. A float keeps its id, terminal, name and icon; it is one pane (no splits inside: `pane.split` on a float splits the tiled pane it would dock beside). Floats are part of the tab's structure: kept in the machine's document with their rects and order, so every screen of the machine shows them; focus and zoom stay this screen's. Floats always draw above the tiled panes, also over a zoomed tiled pane; `pane.zoom` on a float fills the tab with it on this screen. `pane.go` reaches floats (the front one wins a tie) when one lies in that direction nearer than a tiled pane; `pane.swap {target}` between a float and a tiled pane trades their places, `pane.swap {direction}` moves tiled panes only, and `pane.arrange` leaves floats where they are. Float commands on a tiled pane fail ("doesn't float"). `panes()[i].floating` is `{rect, z, hidden}` for a float (`z` 0 at the back), else null; `herdr-app ls` gives a float's terminal its `floating` rect. The shipped `menu` offers Float pane, or Dock pane and Bring to front, and the shipped `pane` header has a float button among its hover buttons (Float, or Dock on a floating pane: `pane.float_toggle`). Default keys: Cmd+Alt+Enter float or dock, Cmd+Ctrl+arrows move, Cmd+Ctrl+Shift+arrows resize (macOS); unbound elsewhere.
  `pane.zoom_text_in {pane?}`, `pane.zoom_text_out {pane?}`,
  `pane.zoom_text_reset {pane?}` zoom one terminal pane's text (default:
  the terminal with the keyboard, else the focused pane) a `zoom_step` at
  a time, on top of the app's `zoom`, as Ghostty's per-surface font size
  does: its program gets the new rows and columns, and other panes and the
  panels don't change. The size is `font_size × its zoom × zoom`, kept
  within `font_size × 0.5` to `× 3`; resetting either zoom keeps the
  other. It lasts as long as the pane and isn't saved; `panes()` has the
  pane's `text_size` while it differs, and the shipped `pane` header shows
  it ("16pt"). macOS binds Ctrl+= (and Ctrl++), Ctrl+- and Ctrl+0 in
  `global`; other platforms leave them unbound, since Ctrl+= is
  `app.zoom_in` there. With `pane_zoom_keys` off, or no terminal holding
  the keyboard, those keys reach the program; object-form bindings
  (`{"command": "pane.zoom_text_in"}`) follow it too.
  `pane.go` prefers the most recently focused pane along the nearest edge in that
  direction; unvisited panes fall back to overlap, then top/left layout order.
  `pane.go` also reaches visible mods whose `mod.json` says `"focusable": true`, and
  starts from that mod while it holds the keyboard.
  `pane.swap {direction}` uses geometry only. Focus history lasts until the window closes.
  `pane.swap {target}` trades the places of `pane` (default: the focused one) and
  `target`, another pane of the same tab; give `direction` or `target`, not both.
  A swap moves panes, not focus: the focused pane stays focused in its new place.
  `pane.arrange {mode, tab?}` rebuilds a tab's panes (default: the shown tab) with
  even sizes: `tiled` (a grid row by row, `ceil(sqrt(n))` across; a short last
  row's panes widen), `bsp` (each pane takes half of what's left: right, down,
  right…), `master` (the focused pane on the left half, the rest stacked on the
  right), `columns`, `rows`, `next` (the mode after the tab's last, from
  `tiled`; the tab's `arrange` field says which), or `off`. Panes go in reading order: by
  top edge, then left edge. Terminals, mod panes, names and focus stay; a zoomed
  pane unzooms. The new tree is saved like any split. The tab keeps the mode
  (on this screen): every pane split into the tab or closed in it, by a key, a
  mod or an agent, arranges it again with even sizes, the new pane right after
  the one it split in reading order (in `master` the master stays). A dragged
  divider holds until the next split or close; `pane.swap` and pane drags keep
  the mode, and the next arrangement follows the new order. `bsp` is the
  exception, as in tuios and bspwm: after the first arrangement a split halves
  the pane it splits in the direction asked, a closed pane's space goes back to
  the pane it was split with, and dragged sizes stay; a terminal added for an
  agent splits across the parent of the pane it splits, so several dwindle. `off` clears the mode and leaves the panes
  where they are; `next` never picks it. settings.json's `default_arrange`
  gives new tabs a mode. Default key: Cmd+Shift+Space (macOS), Ctrl+Shift+Space
  (Linux, Windows).
  `focus.toggle_follows_mouse` turns `focus_follows_mouse` (below) on or off in
  settings.json and says which in a toast; when settings.json has errors or
  can't be written, the change applies unsaved, the toast warns why, and a
  control-socket caller gets that as an error.
  `pane.focus {terminal}` prefers the first visible layout terminal block showing that
  address, then its model pane. Clicking a terminal block focuses it too.
  While a layout terminal block is focused, implicit split/close/zoom/go/swap and
  `terminal.show` fail, as do `terminal.open`/`terminal.keep` with place right/down.
  They never act on the terminal's source pane. An explicit `pane` still targets that
  model pane; composition edits belong to the mod (edit its layout instead).
- Terminals: `terminal.open {argv?, folder?, place?, name?}` (`folder` sets the
  starting directory for tabs and right/down splits, with or without `argv`), `terminal.kill {terminal}`,
  `terminal.keep {terminal, place?}`, `terminal.park {terminal}`,
  `terminal.peek {terminal}` (peek over the tab, with keep-as-tab and close),
  `terminal.popup {argv, cwd?, hold?, wait?, width?, height?}` (start a program on
  this computer without a parent and peek at it in one step; the terminal is removed
  when it ends unless `wait`, which keeps it until `{"wait": terminal}` on the
  control socket returns its `exit_code` (and `signal`), or for a minute; `hold`
  keeps the popup up until a key; ✕ hides the popup and the program keeps
  running until the next popup, which ends the program the last popup started,
  open or hidden; `herdr-app popup` wraps it),
  `terminal.show {terminal?, pane?}`, `terminal.search` (screen and scrollback;
  find bar state stays with that terminal), `terminal.copy {terminal?}` (the selection), `terminal.copy_all {terminal?}` (the screen and all the history the core keeps, as plain text: what `hc read` prints, without the blank lines below the last text; a toast says how many lines), `terminal.copy_path {terminal?}` (the folder the terminal is in), `terminal.copy_last_output {terminal?}` (the output of the last command that finished, wrapped lines joined, without trailing blank lines; see "Copy last output" below), `terminal.paste {terminal?}` (copied
  files, such as Finder's or a clipboard manager's image, as if dropped; an image
  alone as `terminal.paste_image` pastes it), `terminal.paste_image` (Ctrl+V: an
  image, into a terminal on another machine: copied there, its path pasted; on
  Windows, clipboard text is pasted; otherwise Ctrl+V goes to the program, which
  reads the clipboard itself). TIFF and BMP clipboard
  images are converted to PNG before sending,
  `terminal.right_click {to: "program" | "menu", terminal?}` (where right-clicks go
  when the program takes the mouse; kept as its label `herdr.right-click=program`;
  Shift+right-click is always Herdr's menu),
  `terminal.scroll {lines | pages | to: "top" | "bottom", terminal?}` (moves Herdr's view of the
  history, not the program: negative lines or pages go up, toward history; pages may be
  fractional, one page is the terminal's rows; on the alternate screen, such as vim or less,
  there is no history, so nothing moves),
  `terminal.pin {terminal, pinned}` (pins an agent to the top of the agent lists;
  kept on the terminal as `herdr.pinned`, so every Herdr client shows it, and
  gone when the terminal ends; read it as `agents()` `pinned`). Without `terminal`, copy, paste, paste-image and implicit
  right-click targeting follow keyboard focus, including peek and floating terminals;
  with no terminal holding keyboard focus, they use the focused pane or block.
  Dragging a text selection past the terminal's top or bottom scrolls while held;
  releasing stops it. Programs that track the mouse still get their mouse events
  (hold Shift to select text instead). A drag stays with the terminal where it
  started, including its release outside the pane; other panes don't receive it.
  Terminals display Kitty graphics, including Unicode placeholder images used by
  programs such as yazi; partial images and scrolling follow the placeholder cells.
- Copy last output: `terminal.copy_last_output` and the pane menu's "Copy last output" copy what the last finished command printed, read from the marks a shell writes around its prompts (OSC 133: `A` prompt start, `B` input start, `C` output start, `D` command end). The menu shows the item only when there is such output. It covers output in the scrollback (as far as Herdr's view of the terminal keeps), joins lines the terminal wrapped, and drops trailing blank lines. A command still running doesn't count: the one before it does. Enter on an empty prompt is skipped; a command that printed nothing, or a half-typed line left with Ctrl+C, leaves nothing to copy. There's nothing on a full-screen program (the alternate screen), in a terminal whose shell writes no marks (most agents and TUIs), or for a command typed at a prompt drawn before Herdr attached to the terminal (the core's first paint carries no marks: after Herdr restarts, the first command in a terminal doesn't count, the ones after it do). Shells write the marks through shell integration: fish does by itself, oh-my-posh does with `shell_integration = true` in its config. A terminal app's integration (Ghostty's, kitty's) reaches only shells that app starts, not Herdr's. Plain zsh needs two hooks in `~/.zshrc`:

  ```zsh
  autoload -Uz add-zsh-hook
  _osc133_precmd() {
    print -n "\e]133;D;$?\a"
    [[ $PROMPT == *$'\e]133;A\a'* ]] || PROMPT=$'%{\e]133;A\a%}'"$PROMPT"$'%{\e]133;B\a%}'
    [[ -z $RPROMPT || $RPROMPT == *$'\e]133;P'* ]] || RPROMPT=$'%{\e]133;P;k=r\a%}'"$RPROMPT"$'%{\e]133;B\a%}'
  }
  _osc133_preexec() { print -n "\e]133;C\a"; }
  add-zsh-hook precmd _osc133_precmd
  add-zsh-hook preexec _osc133_preexec
  ```

  Bash: `PS0=$'\e]133;C\a'`, `PROMPT_COMMAND='printf "\e]133;D;%s\a" $?'"${PROMPT_COMMAND:+;$PROMPT_COMMAND}"` (first, so it reads the command's `$?`) and `PS1=$'\[\e]133;A\a\]'"$PS1"$'\[\e]133;B\a\]'`. The marks go inside `PROMPT` and `RPROMPT`, not printed once from precmd, so a prompt zsh redraws (a resize, `reset-prompt`) and a right prompt stay marked as prompt rather than as typed input. Register the zsh hooks after any prompt tool that rebuilds `PROMPT` each time (oh-my-posh, starship), so the marks are added to its prompt.

  A transient prompt (the short prompt left in the history once a command is entered) is a redraw precmd never sees: the prompt tool sets `PS1` inside the line editor and redraws. Unmarked, it reads as typed input, so after Enter on an empty prompt there is nothing to copy. Mark its `PS1` too. For oh-my-posh's `[transient_prompt]`, after `oh-my-posh init` and the hooks above, with `_osc133_mark_prompt` holding the two `PROMPT`/`RPROMPT` lines of `_osc133_precmd`:

  ```zsh
  if [[ $functions[_omp_get_prompt] != *_osc133_omp_get_prompt* ]]; then
    functions[_osc133_omp_get_prompt]=$functions[_omp_get_prompt]
    _omp_get_prompt() {
      _osc133_omp_get_prompt "$@"; local ret=$?
      [[ $1 == transient ]] && print -r -- $'\n_osc133_mark_prompt'
      return ret
    }
  fi
  ```

  The guard reads the current body, so sourcing `.zshrc` again (which re-runs `oh-my-posh init` and redefines `_omp_get_prompt`) wraps the fresh one, and a second wrap can never make the wrapper call itself.

  Starship's transient prompt (a zsh plugin or your own `zle-line-init` widget) needs the same: wrap the `PS1` it sets in `%{\e]133;A\a%}`…`%{\e]133;B\a%}` before it redraws.
- Agents: `agent.next`, `agent.prev`, `agent.focus {terminal}`.
- Notifications: `system.notify {title, body?, subtitle?, sound?, agent?, id?, urgent?, icon?}` posts a notification as Herdr for any program: `herdr-app do system.notify '{"title": "Build", "body": "passed", "sound": "Glass", "id": "build"}'`. `sound` as one `notification_sounds` value (default `"default"`); `agent` (a terminal address, or an agent's name when only one has it) makes a click show that terminal and attaches its program's icon (unless `icon` is given or `notification_icons` is `false`); a newer one with the same `id` (1 to 64 of `A-Z a-z 0-9 . _ : -`) replaces the older; `icon` is an absolute path to an image up to 10 MB (megabytes): PNG (Portable Network Graphics), JPEG (Joint Photographic Experts Group), GIF (Graphics Interchange Format) or HEIC (High Efficiency Image Container); Herdr attaches a copy. It follows `system_notifications`: `off` fails; `background` while Herdr is the app in front shows the window's toast instead (answer `{"shown": "toast"}`) unless `urgent: true`; otherwise it posts (answer `{"shown": "notification", "tag": ...}`). Title and subtitle are cut at 200 characters and the body at 2000; control characters (the body keeps its line breaks) and direction overrides are left out. It fails, exiting 1, where macOS won't show it: notifications for Herdr turned off in System Settings, or a build not run from Herdr.app.
- Sessions: `session.saved` lists the recorded agents (programs with a `resume` rule in `programs.json`) with `terminal`, `where`, `program`, `name`, `cwd`, the resuming `argv`, `session`, `saved_ms`, and `ready` (its terminal is live and back to a bare shell) or `why` not. `session.resume {terminal?, confirmed?}` types `cd <cwd> && <argv>` (each word shell-quoted; PowerShell: `Set-Location`) into every ready one (or just `terminal`) and answers `{resumed, skipped: [{terminal, why}]}`; it asks the person first unless `confirmed`, and scripts must pass `confirmed: true` (`herdr-app do session.resume --confirmed true`). It never types into a terminal whose foreground isn't a shell, and skips a session two terminals share.
- Layout: `layout.switch {name?}`, `layout.toggle {id}`, `layout.show {id}`, `layout.hide {id}`,
  `layout.resize {id, px | weight}`, `layout.select {stack, child}`, `layout.set {layout}`
  (personal tree edits; on shipped layouts only click/look fields change),
  `layout.reset {name?}` (reset a shipped layout and its clicks),
  `layout.spacing {pane_gap?, area_padding?, area_gap?, terminal_padding?, reset?}`
  (space around the shown tab's panes as clicks, px 0 to 40; `area_*` is the row or
  column holding the tab block, `terminal_padding` `[x, y]` the root's; `reset` lists
  names to put back as the file has them),
  `layout.clone {name, as?}` (duplicate, then switch; default `<name>-copy`).
- App: `mod.clone {name, as?}` (duplicate; default `<name>-copy`), `mod.open {name, props?}` (as the overlay its mod.json describes, `props` being what its `props()` holds; again: closes it, or opens it afresh with other props), `overlay.close {id?}`, `app.help` (again: closes it), `app.palette` (again: closes it), `app.settings {section?}` (a Settings page: `themes`, `layout`, `window`, `panes`, `zoom`, `fonts`, `grid`, `rendering`, `keyboard` (macOS), `mouse`, `activity`, `alerts`, `machines`, `updates` or `privacy`; `look` and `text` are aliases for Themes and Zoom)
  (Machines lists, updates, renames and removes ssh machines; settings edit the same files you do; its search field, focused by cmd-f or ctrl-f or by typing, finds a setting by its name, description or settings.json key, such as `cell_rounding`, and shows each page's matches with their controls).
  `app.check_updates` (only a successful check reports Up to date; a failed check clears the checked time and can be retried), `app.update_core` (a newer herdr-core of this generation, terminals kept;
  never one a package manager owns), `app.welcome`, `app.whats_new`, `app.feedback {message?, from?}` (from `"person"` or `"agent"`; opens a review window, where the person can paste up to 3 screenshots; never sends until the person clicks Send), `app.reset`,
  `app.theme {name}`, `app.zoom_in`, `app.zoom_out`, `app.zoom_reset` (the whole app; a terminal pane's own text: `pane.zoom_text_*`),
  `app.update` (restart into a downloaded update).
  Add machine lists all named ssh-config hosts in a scrollable list. Machines
  that need a password, a new host key or herdr-core installed show what to run
  in your own terminal; the picker updates once it is added. Other machines are
  added quietly. If a remote machine was rebuilt
  or replaced and a different core answers there, its sidebar offers Connect:
  explicitly forget the old identity and pair again with the same name and target.
  Other unreachable machines don't offer this.
- Window: `window.minimize`, `window.maximize` (or back), `window.close`.
- macOS: `app.quit`, `app.hide`, `app.hide_others`, `app.show_all`.

When the user runs a command without a required name (a key, a click), Herdr
asks them. **You never get a dialog**: pass every required argument, or the
command says what's missing.

From a shell:

```sh
herdr-app do tab.new --name "fix login" --argv '["claude"]' --task "look at the login bug"
herdr-app do pane.split:right
herdr-app ls          # spaces, their tabs (with each tab's arrange mode) and terminals with their label and session (sub-agents under their parent)
herdr-app status      # what's on screen: layout, focus, overlays, zoom, errors from files and mods
herdr-app commands    # every command and its arguments
herdr-app popup --wait -- make test   # a command in a popup; exits with its exit code
```

`herdr-app do` prints what the command made (`{ "space", "tab",
"terminal" }`, `{ "pane", "terminal" }`) or `{ "error": "..." }` and exits 1.

## The person: `settings.json`

What's about the user, never a look:

```json
{ "zoom": 1, "sounds": { "enabled": true } }
```

- `zoom` scales the whole app: terminals, mods, the layout's px sizes
  (0.5 to 3; the zoom keys change it). Write sizes as designed at zoom 1.
- `zoom_step`: what one press of the zoom keys moves. `"tenth"` (default) steps `zoom` by 0.1, so 12 pt text goes 13.2, 14.4, 15.6. `"point"` steps the terminals' text a whole point at a time, as Ghostty's font size does (12 to 13, 14, 15), by setting `zoom` to the new size over `font_size`; the panels scale by the same factor. From between two points (a zoom set in tenths), the first press goes to the nearest whole point in its direction. Reset still goes to zoom 1. Changing `font_size` (or switching to a layout with its own) keeps `zoom`, so the text can sit between points until the next press.
- `pane_zoom_keys` (default true): keys bound to `pane.zoom_text_in`, `pane.zoom_text_out` and `pane.zoom_text_reset` (Ctrl+=, Ctrl+- and Ctrl+0 on macOS) zoom the focused terminal's text. `false` leaves those keys to the program; the commands still run from the palette, mods and `herdr-app do`. The Settings window's Zoom page has it under Zoom steps ("Pane zoom keys").
- `ui_zoom` (0.75 to 1.5, default 1): the panels only, on top of `zoom`: the layout's px sizes, pane headers, tabs, overlays, popups, toasts and every mod's px (text, spacing, icons). Terminals keep their text, cells and padding (`zoom` alone). A mod's px are drawn at `zoom × ui_zoom` (at most 3): divide window px by `settings().ui_scale`, which is that product. `settings().ui_scale_ranges` has both ranges.
- `ui_text_scale` (0.8 to 1.3, default 1): the panels' text only, on top of their zoom: mods' `text_size(...)` (px or `"1rem"`), px and rem `line_height`s, editor `font_size`, and Herdr's own panel text. The window's rem leaves it out, so rem spacing, icons, row heights and terminals keep their size, and a big scale can crowd a fixed-height row.
- `sounds`: `{ "enabled": true, "done": "<file>", "blocked": "<file>" }`.
  When an agent needs the person, or finishes while they aren't looking at
  it, Herdr plays a sound and (unless they're looking) posts a toast that
  goes there when clicked. Alerts of one kind within 3 seconds of the last
  share one toast ("3 agents finished", names newest first; a click goes to
  the newest). `done` / `blocked` replace the bundled MP3s
  (path absolute or relative to `~/.config/herdr-app`). Changes apply live,
  including the Play buttons in Settings > Alerts.
- `resume_agents`: `"ask"` (default), `"auto"` or `"off"`: what happens after Herdr starts or reconnects to the core when recorded agents (see `resume` in Programs) are in terminals back to a bare shell, as after a reboot. `ask`: one toast, "Resume N sessions", whose click runs `session.resume`; `auto`: resumes them at once; `off`: leaves the shells (records are still kept, and `session.resume` still works). Settings > Agents > Activity has it under After a restart, as "Resume agents after restart".
- `system_notifications`: `"background"` (default), `"always"` or `"off"`.
  The same alerts also go to the system's notification center: titled as
  the toast, with "space / tab" as the body, one per agent (a newer alert
  replaces the older one). `background` sends them only while Herdr isn't
  the app in front; `always` also while it is. Never for the agent the
  person is looking at. Clicking one brings Herdr forward on that agent.
  A "needs you" one stays until the agent is no longer blocked (see
  `notification_sticky`); the others go once the person looks at
  the agent or it starts working again. The toast shows either way. macOS asks permission the first
  time; a build not run from an app bundle sends none.
- `notification_sounds`: the sound each system notification plays (macOS). Absent: silent, as macOS plays none unless asked. One value for every state, or one per state: `{"done": "Glass", "blocked": "Purr", "error": "Basso"}` (a state left out is silent). Each value is `"default"` (the system's notification sound), `"none"` or `false` (silent), `true` (the system's), or a sound's name: a file in `~/Library/Sounds`, `/Library/Sounds` or `/System/Library/Sounds` without its extension (`Glass`, `Purr`, `Basso`, `Tink`, `Ping`, `Hero`, ...). Up to 64 letters, digits, spaces, `-`, `_` and `.`; anything else, or another key, leaves them silent and is reported as a settings problem. A name macOS can't find plays the default sound. These are the notification's own sounds, beside Herdr's `sounds`, which play in the app whether or not a notification goes. Settings > Alerts > Notifications has it as "Notification sound".
- `notification_marks`: `true` puts the agent's glyph before an agent notification's title (Claude 🤖, Codex 🧠, Gemini ✨, Cursor 🖱️, Copilot 🚀, Aider 🛠️, Crush 💜, OpenCode 📂, Amp ⚡, others 🤖) and the state's mark before its body (✋ needs you, 🔴 error, ✅ finished): "🤖 Claude needs you" over "✋ approve Bash: make". Off by default; Settings > Alerts > Notifications has it as "Marks".
- `notification_icons`: the agent's program icon (programs.json's `icon`, drawn as the agents list draws it: a one-color glyph in its `color` on its `tile`, or a picture) shows with its system notification on macOS. Drawn once per program and look into `cache/notification-icons/` in the home folder. A program without an icon, or one that can't be read, shows none. On by default; `false` leaves them off. Settings > Alerts > Notifications has it as "Agent icons".
- `notification_sticky`: a "needs you" system notification (an agent blocked on a question, a permission or a login) stays in the notification center until the agent leaves blocked or error: it reports working, idle or done, stops reporting, or its pane closes or its terminal ends. It goes when the program exits back to its shell only once the core reports that change; until then the agents list still shows it as needing you. One still up when Herdr quits or restarts is picked up again on its next start. Looking at the agent doesn't take it down, so one seen in passing is still there to come back to. Blocked again later, the agent's new notification replaces the old one. Error and finished notifications still go once the person looks at the agent. On by default; `false` takes a "needs you" notification down once seen, as the others are. settings.json only.
- `dock_bounce`: `true` bounces the Dock icon when an agent needs the
  person while Herdr is in the background. Off by default. Settings >
  Alerts > Notifications has it as "Bounce the Dock icon".
- `dock_badge`: on macOS the Dock icon counts the agents waiting on the person, the way Mail counts unread mail: each row the agents list shows as needing you (blocked on a question, a permission or a login, or stopped on an error, its own or a sub-agent's) or as finished and not yet seen. Looking at a finished agent, or it starting work again, takes it off the count; no badge when none waits. On by default; `false` takes the badge off. Settings > Alerts > Notifications has it as "Count waiting agents on the Dock icon".
- `dock_bounce_on_wait`: `true` bounces the Dock icon once (macOS's short, informational bounce) when an agent starts waiting and none was, while Herdr is in the background. Off by default.
- `pane_auto_names`: `true` gives each pane nobody named two calm words of
  its own, such as `hazy-inlet`, picked from its id so they last across
  redraws and restarts. Two panes of one tab never share them: the one
  whose id sorts later takes the next pair, so a split can move an
  existing pane's name on until it closes. A pane
  shows them only at a bare shell whose title says nothing more (empty,
  its address, the shell's name, a folder such as `~/src`); a name given
  with `pane.rename`, an agent or a running command comes first. Mods read
  `panes()[i].auto_name` (null when off or not shown); the shipped `pane`
  and `terminals-pane` headers show it. Display only: never written into a
  layout or settings.json, though `pane.rename`'s prompt starts from it.
  The words are `defaults/pane-names.json` in the app. Off by default;
  Settings > Appearance > Panes has it as "Name panes automatically".
- `space_avatar`: how mods that draw space avatars (a space's tile in a spaces list) draw them: `"letter"` its first letter on its color, `"glyph"` (the default) a one-color icon on its color, `"emoji"` an emoji on a neutral tile, `"repo"` the language or tool of the project in its folder (else a glyph). Herdr also uses it for pane and tab icons: in glyph, emoji or repo (glyph) mode, a bare shell showing its `pane_auto_names` name gets its noun's icon (meadow: wheat; the nouns' icons are `defaults/pane-icons.json` in the app), as `panes()[i].icon`, and a tab shows its focused pane's; in letter mode `icon` is null unless one was chosen (pane.set_icon, tab.set_icon), so panes and tabs draw their program's icon. Mods read the setting as `settings().space_avatar`, `"glyph"` when unset; a value other than those four is reported as a settings problem and mods see `"glyph"`. The Settings window doesn't set it. For glyph and emoji avatars that match a space's name, read the icon picker's name files once, when first needed: `readTextFile(appHome() + "/mods/icon-picker/names/keywords.json")` (`icons: [{words, glyph, emoji}]`: curated words such as docs, api or infra, a glyph by its `glyphs.json` name such as `md-book_open_variant`, an emoji as `emoji.json` writes it, each word once; `glyph_sets`: the glyph name prefixes to match, best first; `no_emoji_groups`: emoji groups to leave out; `deny: {words, emoji}`: words and emoji that read as an agent's state, such as check, done or a warning sign, never to match), then `glyphs.json` and `emoji.json` for words that are a glyph's or an emoji's own name; a name matching nothing can fall back to a hash of the name, settled in an order that doesn't depend on the list's.
- `working_spinner`: the spinner shown while an agent works, in the pane header, tabs, agents list, picker and peek. `"ring"` (the default) is the spinning ring icon. Presets, each at its own speed: `"braille"` ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏, `"braille-heavy"` ⣾⣽⣻⢿⡿⣟⣯⣷, `"orbit"` ⠁⠂⠄⡀⢀⠠⠐⠈, `"line"` | / - \, `"arc"` ◜◠◝◞◡◟, `"quarters"` ◴◷◶◵, `"pulse"` · • ● •, `"sparkle"` · ✢ ✳ ✶ ✻ ✽ and back. Or your own: `{"frames": ["◐", "◓", "◑", "◒"], "interval_ms": 120}`, 1 to 32 frames of 1 to 4 characters (Unicode code points) each, 16 to 1000 ms apart. An unknown name or a frames object outside those limits draws the ring and is reported as a settings problem. Mods read it as `settings().working_spinner`: `null` for the ring, else `{ frames, interval_ms }` with the frames padded with spaces to one length, and `settings().working_spinner_presets` lists `{ name, frames, interval_ms }` for each preset besides the ring (for a picker); draw it as `div().font_family(settings().font_family).frames(frames, interval_ms).child(frames[0])` in `t.working`, centred in a box at least 12 px wide, and the ring as `svg("mods/workspace-chrome/spinner.svg").size(12).spin(900)`. Settings > Agents > Activity has it as "Working spinner".
- `pane_header_info`: extra words in an agent pane's header, between its
  name and its status, as a template. Unset or empty (the default): none.
  Keys: `{ask}` what the agent needs from you while it waits ("approve
  Bash: make", else "permission", "question"), `{state}` and `{elapsed}`
  (`<1m`, `3m`, `1h2m`, changing on the minute), and `{branch}`, `{ahead}`
  (`↑2`), `{behind}` (`↓1`) and `{dirty}` (`*`) from the folder's cached
  git status, `{subagents}` the agent's live sub-agents (`3 sub-agents`, `3 sub-agents, 1 blocked`,
  `2 sub-agents blocked`), and `{machine}` (`local` for this computer), `{remote}` (another machine's name, else empty) and `{space}` (its space's name); each is empty when unknown or zero, and other text is drawn
  as written. Keys side by side (`{branch}{dirty}`) are drawn a space
  apart ("main ↑2 *"). `fitHeaderInfo` measures each separator as
  `sep.trim()` with 4 px either side, so draw it that way, as the shipped
  `pane` header does (`segments[i].sep` keeps it as written, for a tooltip). `·` or `|` split it into segments: one whose keys are all
  empty is left out, and where the header is short of room whole segments
  go from the right, `{subagents}` first and a waiting `{ask}` or blocked
  `{subagents}` last. Suggested:
  `"{ask} · {branch}{ahead}{behind}{dirty} · {state} {elapsed} · {subagents}"`. Shells
  show nothing. Mods read it resolved as `PaneInfo.header_info` and keep
  what fits with `fitHeaderInfo`. Settings > Agents > Activity has it as
  "Header info".
- `agents_card_info`: what an agent's card in an agents list shows beside its name, as a template in `pane_header_info`'s grammar and keys. Unset, the default `"@{remote} · {space}"`: another machine's name, then the space; set empty, nothing. Herdr resolves it for each agent when what it shows changes (and on the minute when it has `{elapsed}`), never per frame, as `agents()[i].card_info`: `{ text, segments }` as `header_info`, null when nothing shows, and with `{space}` in the template also `no_space`, the same without the space (null when nothing is left), for a card whose space goes without saying (under its space's header, or when its name is the space); a segment that holds other keys keeps them there. Draw each segment as a chip or words, in order; `card_info` is undefined on builds without it. Mods read the template as `settings().agents_card_info`, `null` while unset: the shipped `agents` mod draws `card_info` (without the space, which its rows always name) in place of the machine only once it is set, so its rows look as they always have by default. Settings > Agents > Activity has it as "Card info", with a preview on an agent on another machine.
- `agents_subagents`: how the agents list shows each agent's live sub-agents. `"tree"` (the default) a row each under it, with their count on its row and a chevron to fold them; `"folded"` the same with every agent folded until opened; `"off"` no sub-agent rows, count or chevron, and a sub-agent with a pane keeps its own row (a blocked sub-agent still moves its parent into Needs you, as the Dock badge counts it). A fold or unfold the person makes against the setting is kept across restarts in the mod's localStorage (one that matches the setting is dropped, so a change of setting reaches it), until the agent is unlisted and has not changed state for 30 days; `agents.fold_all` and `agents.unfold_all` (keys.json, the palette's Fold all agents and Unfold all agents) fold or open every agent listed, and reach the list only while it is on screen. `pane_header_info`'s `{subagents}` doesn't follow it: that key shows wherever the template has it. Mods read `settings().agents_subagents`, `"tree"` when unset. Settings > Agents > Activity has it as "Sub-agents".
- `agents_subagent_click`: what a click on a sub-agent row in an agents list should open. `"claude"` (the default) the sub-agent in Claude Code's own view (a mod can get there by sending the agent's own keys, after checking it is the one shown), `"transcript"` a popup with the sub-agent's transcript. Herdr only passes it on: it is a preference agents mods may honour, and the shipped `agents` mod doesn't read it (a click on a sub-agent row still focuses the parent agent's pane, or peeks it when that agent isn't listed). Mods read `settings().agents_subagent_click`, `"claude"` when unset. Settings > Agents > Activity has it as "Sub-agent click", after "Sub-agents".
- `focus_follows_mouse`: `true` focuses a terminal pane (a tab's, or a layout's
  terminal block) once the pointer rests on it for `focus_follows_mouse_delay_ms`
  (0 to 2000, default 150), as a click would: the delay counts from the
  pointer's last move, so a pane it only crosses on the way somewhere else is
  left alone; at 0 the first move onto a pane focuses it. Off by default;
  `focus.toggle_follows_mouse` flips it, and Settings > Input > Mouse has
  it, with the delay while it's on. Only a real pointer move counts:
  content scrolling or panes moving under a still pointer never move the focus,
  nor does a jitter under 1 px. After a key, or the focus moving some other way,
  the pointer must first go more than 4 px from where it was, so a pane focused
  from the keyboard keeps it while the hand rests on the mouse.
  Nothing happens while a mouse button is held, while a dialog, popup, peek or
  menu is open, or while Herdr isn't the active window. Toasts and window
  overlays don't stop it, but a pane under a window overlay isn't focused
  through it. Mod blocks, the sidebar and the tab bar never take the focus this way.
- `default_arrange`: the `pane.arrange` mode tabs made on this screen start
  in (`tiled`, `bsp`, `master`, `columns` or `rows`), so their splits and
  closes arrange them from the first split (`bsp` splits the pane asked, the
  way asked). Absent (the default), or any other
  value: new tabs have none. Tabs already open keep theirs. The Settings
  window's Panes page sets it ("New tabs arrange").
- `alert_command`: a program run for each alert in place of the system
  notification, under the same `system_notifications` rule: one string
  (run by `/bin/sh -c`) or an argv array (`["notify", "-q"]`). Herdr
  doesn't wait for it, and gives it `HERDR_AGENT_STATE` (`blocked`,
  `error` or `done`), `HERDR_AGENT_PREV_STATE` (the status it left, when
  known), `HERDR_AGENT_APP` (e.g. `claude-code`), `HERDR_AGENT_NAME`,
  `HERDR_AGENT_ADDRESS`, `HERDR_AGENT_PLACE` ("space / tab"),
  `HERDR_ALERT_TITLE` (the toast's title), and the agent's own words from
  its status report, as given and empty when it said nothing:
  `HERDR_AGENT_MESSAGE` (its summary, e.g. "approve Bash: make"),
  `HERDR_AGENT_KIND` (`permission`, `question` or `auth`) and
  `HERDR_AGENT_TITLE`. They are environment variables only, never part of
  the command line; quote them in a shell line (`"$HERDR_AGENT_MESSAGE"`).
  The system notification shows the same message above "space / tab". It runs with Herdr's
  environment, whose PATH may be short when Herdr was opened from the
  Dock: give a full path. It starts in the home folder; one still running
  after 30 s is killed with everything it started. One that doesn't start,
  or exits non-zero (a shell line naming a missing program exits 127), gets
  the system notification instead, and a toast says why (once per reason,
  and again after `alert_command` changes); one killed by a signal is only
  logged.
- `core`: the herdr-core binary to use, when it isn't where Herdr looks
  (PATH, the login shell's PATH, `~/.local/bin`, Homebrew, mise, Nix,
  cargo). Read at start.
- `font`, `font_size`, `line_height`, `font_weight`, `font_weight_bold`, `letter_spacing`, `ui_font`: used where the layout doesn't set its
  own. Herdr (main) leaves these unset. Bigger panels: `zoom`.
- `font_ligatures`: programming ligatures are off by default; set `true` to
  enable them in terminal fonts such as JetBrains Mono and Fira Code. The cursor
  and background changes (including selections) break ligatures so individual cells stay clear.
- `font_thicken`: how macOS font smoothing draws terminal text. `"auto"` (the
  default) leaves it to the system, which thickens light text on a dark
  background more than dark text on a light one. `true` thickens text the same
  for every text color, as Ghostty's `font-thicken` does. `false` turns font
  smoothing off: plain antialiased text, the same for every text color and
  lighter than either, as Ghostty's `font-thicken = false`. Panels follow
  `ui_font_thicken` instead.
- `font_thicken_strength` (0..255): how much `font_thicken: true` thickens,
  Ghostty's `font-thicken-strength`. Default 255. macOS applies it in a few
  steps: 0 to about 100 all draw as its lightest smoothing, the one dark text
  gets under `"auto"`; 255 draws as white text does. With `font_weight` 400
  and a strength of 30, terminal text matches Ghostty's Regular at strength 30.
  Changes apply immediately, including text already on screen. The Settings
  window sets both on its Rendering page under Terminal text: a Thicken picker
  (Auto, On, Off), and a strength stepper in steps of 16 while it's On.
- `ui_font_thicken` (`"auto"`, `true` or `false`): the same for the panels'
  text (every mod, tab, popup, toast and the Settings window), apart from the
  terminals' `font_thicken`. Emoji are never thickened. `"auto"` (the default)
  keeps the system's smoothing, where dark text on light draws thinnest.
- `ui_font_thicken_strength` (0..255): how much `ui_font_thicken: true`
  thickens, with the same steps as `font_thicken_strength`. Default 255. The
  Settings window sets both on its Rendering page under Interface text: a "Thicken interface
  text" picker (Auto, On, Off), and a strength stepper in steps of 16 while
  it's On.
- `text_contrast` (0..1): sharpens the edges of terminal text without widening its strokes. When the text is blended, each edge pixel's coverage moves toward an S-curve: faint pixels fade and nearly solid ones fill, so a stroke keeps about the same ink. Thickening adds ink instead. 0 (the default) draws text as rasterised; 1 uses the full curve. Try 0.5 first. Panels follow `ui_text_contrast`. Changes apply immediately. The Settings window sets it on its Rendering page under Terminal text, with a "Contrast" stepper in steps of 0.1.
- `faint_opacity` (0..1): the opacity faint (dim, SGR (Select Graphic Rendition) 2) terminal text draws at, as Ghostty's `faint-opacity`. Default 0.5; 1 draws faint text like the rest. Changes apply immediately. The Settings window sets it on its Rendering page under Terminal text, with a "Faint text" stepper in steps of 0.1.
- `font_fallbacks` (a list of family names): fonts terminal text takes a character from, in order, when its own font lacks it, before the system's choice. Default empty. `["ZedMono Nerd Font Mono"]` draws symbols such as ⏺ as Ghostty does with that font as its fallback; bold text takes the fallback's bold face. A fallback glyph keeps its cell, as in Ghostty: in a one-cell character, ink wider or taller than the cell shrinks to fit it, and ink past an edge moves inside; ink already inside its cell is drawn as the font draws it. Nerd Font icons (private use characters) the font lacks don't use this list: they always come from the bundled Symbols Nerd Font, fitted to their cells. Families that aren't installed are skipped. Changes apply immediately. The Settings window doesn't set it.
- `ui_text_contrast` (0..1): the same for the panels' text (every mod, tab, popup, toast and the Settings window), apart from the terminals' `text_contrast`. Icons are left as drawn. Default 0. The Settings window sets it on its Rendering page under Interface text, with an "Interface contrast" stepper in steps of 0.1.
- `unfocused_dim` (0..1): while the Herdr window isn't the active one, it fades this fraction of the way toward the background, so a glance shows which window has the keyboard. What fades is `unfocused_dim_scope`'s choice. The switch is instant both ways: back in Herdr, everything is at full brightness at once. 0 (the default) never dims; try 0.3. The Settings window sets it on its Window page under Inactive window, with a "Dim when inactive" stepper in steps of 0.1.
- `unfocused_dim_scope` (`"window"` or `"text"`): what `unfocused_dim` fades. `"window"` (the default) fades the whole window evenly toward the theme's background: panels, sidebar, tabs, pane headers, terminals with their backgrounds, and every overlay, toast, popup and tooltip on top. `"text"` fades only terminal text (every color, bold and faint included) toward the terminal's background, leaving backgrounds, selection, find highlights, the cursor and the panels as they are. The Settings window sets it on its Window page under Inactive window, as "Dim".
- `sidebar_shade` (`"flat"`, `"active"` or `"always"`): when the sidebars and bars (theme color `chrome`) take a shade darker than their flat color. `"flat"` (the default) never shades; `"active"` shades them while Herdr is the active window and goes back to flat when it isn't; `"always"` shades in both. On a dark theme the shade sits level with `canvas` (the theme's own when it sets one); light themes keep their sidebar, so it has no effect there. A theme, look or settings.json `colors.chrome` keeps that color in every mode. Mods see the color in use through `theme().chrome`. The Settings window sets it on its Window page, as "Sidebar shade".
- `panel_colors` (`"tinted"` or `"classic"`): how the panels derive the colors a theme doesn't set. `"tinted"` (the default) steps each from the terminal's background toward the theme's own foreground, as described under `themes/<name>.json`. `"classic"` is plain white, black and warm grey around the background with translucent borders and a darker sidebar: `canvas` and `chrome` 16% and 32% toward black on a dark theme, toward warm grey on a light one. `sidebar_shade` has no effect with `"classic"`, nor on a theme or look that keeps classic panels (see `themes/<name>.json`); `settings().panels_in_use` says which derivation is drawing. A theme's, look's or settings.json's own `colors` win in both. The Settings window sets it on its Window page, as "Panel colours".
- `toast_position` (`"bottom-right"`, `"bottom-left"`, `"bottom-center"`, `"top-right"`, `"top-left"` or `"top-center"`): where toasts show, the core-update notice with them. `"bottom-right"` (the default) leaves it out of settings.json. A bottom stack grows up, the oldest nearest the edge, and on the right or centre starts above the update pill while that shows; a top stack grows down, the newest nearest the edge, starting below the window's top chrome (the system title strip and the layout's bars along the top, its `window_drag` nodes, as drawn). The Settings window sets it on its Window page, as "Notification position" (the window's top row and bottom row of places).
- `toast_duration` (`"short"`, `"normal"`, `"long"` or `"until-dismissed"`): how long toasts opened from now on stay up (one already open keeps its time). `"normal"` (the default, left out of settings.json) is 5 s, 8 s for errors and warnings (agents that need you or hit an error); `"short"` is 0.6 of that, `"long"` twice it, and `"until-dismissed"` keeps them until clicked (at most 5 still show: a new toast always shows, closing the oldest routine one first, else the oldest error or warning). A toast whose poster set its own `timeout` keeps it (the 1.5 s "Copied"). The core-update notice stays until an update goes through either way. The Settings window sets it on its Window page, as "Notification time".
- `toast_style` (`"standard"`, `"subtle"`, `"high-contrast"` or `"accent"`): how toasts stand off the window. `"standard"` (the default, left out of settings.json) is the derivation under Themes; `"subtle"` sits closer to the panels (the theme's `popover` where it lifts 1.12:1 off a dark background, a border at 1.35:1, a lighter shadow); `"high-contrast"` holds the border at 3:1 on the background and 2:1 on the toast, title and body at 7:1 and the level marks at 4.5:1, with a deeper shadow (it doesn't start from a theme's own `popover`, which may be too grey for 7:1; a `toast` color you set yourself wins, and then text holds 7:1 where that color allows); `"accent"` tints the surface toward the theme's `accent`. Every style keeps the title and body at 4.5:1 or more and the marks at 3:1 or more, and a theme's or settings.json's own `toast*` colors still win. `toastLooks()` has each style's `toast`, `toast_border`, `toast_shadow`, `toast_foreground`, `toast_muted` and level colors in the current theme, and each `toast_corners` radius. The Settings window sets it on its Window page, as "Notification style", drawing each as a small toast.
- `toast_corners` (`"square"`, `"slight"` or `"rounded"`): a toast's corners, 0, 4 or 8 px at zoom 1. `"rounded"` (the default) is left out of settings.json. The core-update notice follows it. The Settings window sets it on its Window page, as "Notification corners".
- `toast_mark` (`"stripe"`, `"dot"` or `"none"`): what shows a toast's level. `"stripe"` (the default, left out of settings.json) is a 3 px stripe in the level's color down the left side and a dot beside the title; `"dot"` the dot alone; `"none"` neither, so only the wording tells the level (an icon a notification brings still shows). Herdr puts it in each toast's props (`props().toast_mark`) when the toast opens, where the toast mod draws it from; a toast already open keeps its mark. The core-update notice follows it. Designs whose shape is their mark ignore it (see `toast_design`). The Settings window sets it on its Window page, as "Notification level mark".
- `toast_design` (`"card"`, `"pill"`, `"terminal"`, `"outline"`, `"icon-card"` or `"banner"`): a toast's shape, in the colors of `toast_style` and the theme. `"card"` (the default, left out of settings.json) is the toast as above. `"pill"`: one 32 px line with round ends, the dot (for a stripe too) or icon and the title; the body shows as its tooltip. `"terminal"`: the terminal's font and square corners, the level in brackets (`[INFO]`, `[DONE]`, `[WARN]`, `[ERR]`) in the terminal's own blue, green, yellow and red, held to 4.5:1 as text; no other mark. `"outline"`: a see-through backing (`toast_ghost`, the toast's surface at the least opacity under which title, body and level colors hold 4.5:1 with 0.1 to spare over black and over white behind it, so over anything (no hover fill paints over it), with the body and level colors stepped at most halfway toward the title color to get there), a 1 px border and title in the level's color, no other mark. `"icon-card"`: 300 to 400 px, a 36 px level icon (its mark: no other), title and body, and Show (the action's `label`) and Dismiss buttons when the toast has an `action`, 76 px high, 112 with buttons; its level colors are held to 4.5:1, as the banner's are, since Show's label is the surface color on them. `"banner"`: a 40 px strip across the window, along the top or bottom edge as `toast_position`'s top or bottom half says, below the top chrome on top; `toast_offset`'s `y` moves it in, its `x` does nothing, square corners, and a bottom banner sits above the update pill. Banners show one at a time, the newest error or warning, else the newest, with "+N more" for the rest and Show and Dismiss buttons; one waiting behind it doesn't count down, and gets its whole time when it comes forward. `toast_corners` applies to card, outline and icon card; `toastLooks().takes` says which designs take corners and marks. Toasts already open take a new design. The Settings window sets it on its Window page, as "Notification design", drawing each as a small toast.
- `toast_progress` (`true` or `false`): a 2 px bar in the level's color along a toast's bottom edge, as wide as the share of its time left. Off (the default) is left out of settings.json. A toast kept until clicked has none. While one shows, the window redraws a step at a time, about 48 steps over the shortest such toast's time and 2 to 10 a second, and not at all otherwise. The Settings window sets it on its Window page, as "Notification countdown".
- `toast_offset` (`{"x": px, "y": px}`, either left out as 0): moves the toast stack in from the window's side (`x`, unused when centred) and from its top or bottom (`y`), at zoom 1, on top of the 16 px margin, to clear a status bar or a layout's own bar; negative values move out, to the window's edge at most. The whole stack moves together, and never under the update pill. Each is kept within -16 to 400; a value outside is brought within and reported as a settings problem. settings.json only.
- `unfocused_dim_color` (`"#rrggbb"`): the color the whole window fades toward while `unfocused_dim_scope` is `"window"`, laid over it at `unfocused_dim`'s alpha. Unset (the default), it's the theme's background. A mid grey such as `"#4d4d4d"` at 0.25 darkens text and lightens backgrounds together, as Rex does. Anything other than a `"#rrggbb"` color is reported as a problem and the background is used. `"text"` scope ignores it. The Settings window doesn't set it.
- `text_rendering` (`"grayscale"` or `"subpixel"`, macOS, trial): how terminal text's edges are smoothed. `"grayscale"` (the default) gives each pixel one coverage. `"subpixel"` gives each of a pixel's red, green and blue stripes its own, so edges sit a third of a pixel finer, with faint color fringes up close. It only shows on 1x (non-Retina), unrotated displays in an opaque window; elsewhere text stays grayscale, and a window moved between displays switches by itself. Subpixel glyphs are drawn by swash rather than CoreGraphics, so `font_thicken` and macOS's own light-text thickening become an emboldened outline of about the same weight, and `font_thicken: false` an outline grown to CoreGraphics' unsmoothed weight. Panels follow `ui_text_rendering`. Changes apply immediately. The Settings window sets it on its Rendering page under Terminal text, as "Text smoothing".
- `ui_text_rendering` (`"grayscale"` or `"subpixel"`, trial): the same for the panels' text, apart from the terminals'. Default `"grayscale"`. The Settings window sets it on its Rendering page under Interface text, as "Interface text smoothing".
- `subpixel_order` (`"rgb"` or `"bgr"`): the display's color stripes, left to right, for subpixel smoothing. Default `"rgb"`, which most displays are; if subpixel text shows colored fringes on the wrong side of each stroke, try `"bgr"`. The Settings window shows a "Subpixel order" picker while either text is subpixel.
- `ui_font_weight` (100..900, default 400): the panels' regular text weight
  (every mod, tab, popup, toast and the Settings window). Text that sets its
  own weight (a mod's `font_weight(...)` or `font_bold()`, bold labels,
  markdown bold) moves by as much, within 100..900, so bold stays bolder:
  at 500, `font_weight(600)` draws at 700. Terminals keep `font_weight`.
  A variable `ui_font` draws every step; static faces draw the nearest weight
  they have (the bundled Geist has only Regular). Changes apply live. The
  Settings window sets it on its Fonts page under Interface text:
  an "Interface font weight" stepper in steps of 25.
- `theme`: a theme name or `{light, dark}`; used where the layout sets none.
  The theme picker overrides any layout and stays on that layout.
- `colors`: theme color overrides (`{"accent":"#ff8800"}`), with the same
  precedence as fonts.
- `appearance`: `"light"` or `"dark"` always; absent, the system's. A layout
  with a light and a dark theme shows the matching one.
- Updates: an installed app downloads them by itself and offers Restart and
  What changed. If the app crashes or is killed with a verified newer update
  waiting, the next launch installs it before opening a window. The first
  start after an app update also updates herdr-core
  to the latest release of the same generation, keeping terminals running.
  Settings → Updates keeps app and core actions together;
  its What's new button opens the waiting update's release range, or this
  version's notes, in the What's new window. Reading never clears the waiting-update dot.
  Rare spotlight releases open What’s new after restart, once; otherwise a
  quiet Updated pill lasts for that launch. A build from source never updates.
  Curated notes come from the bundled root `CHANGELOG.md`; remote notes and
  media are optional. `update-notes.json` records the last successful app
  version and presented spotlights; `update-notes-cache.md` caches remote
  notes; `update-notes-media/` caches HTTPS images (including animated WebP clips),
  referenced below its item in the changelog as `![label](https://...)`.
  Each version uses `### New` with `- **Headline.** detail` highlights and
  `### Fixed` with short bullet items. Unsectioned bullets are plain items.
  Notes expose `items` with `headline`, `detail`, `fixed`, and cached `media`
  objects (`path`, pixel `width`, pixel `height`). Highlights use matching cards:
  edge-to-edge media at its original aspect ratio, then headline and detail.
  For local release-media previews only, launch with
  `HERDR_NOTES_MEDIA_DIR=/var/tmp/herdr-notes-media`: URLs under
  `https://media.herdr.dev/<version>/<scene>.webp` use that directory's
  `<version>/<scene>.webp` instead of downloading. Without the variable,
  media uses the normal HTTPS cache.
  Mods use `updates().notes` and `presentUpdateNotes([version])` only
  for spotlight entries actually shown expanded, not collapsed headlines.
  macOS releases require Apple Silicon; Intel Macs must build from source.
  Keep signed macOS bundles unchanged: don't edit their Info.plist or
  re-sign them. On launch, an installed macOS bundle creates or repairs its
  own `~/.local/bin/herdr-app` symlink, leaving personal files and other
  apps' links alone. Disk-image and translocated launches don't create it.
  Finder launches load the login shell's PATH, with the usual install
  folders as a fallback.
- `option_as_alt`: macOS only, `"auto"` (default), `"on"`, `"off"`, `"left"`, or `"right"`.
  Available in Settings → Keyboard.
  `"auto"` sends Option as Alt on US, US International, ABC and ABC-India;
  other layouts keep native Option text and dead keys. `"on"` enables terminal
  Alt shortcuts (including Option+E without composition); `"off"` keeps native
  Option input on every layout. `"left"` / `"right"` makes only that Option
  key act as Alt; the other types characters. Changes apply immediately, including layout
  switches. Linux and Windows are unchanged.
- `minimum_contrast`: terminal text contrast ratio, `1` (default) leaves
  program colors unchanged. Set it to `1.6` to enable the previous readability
  adjustment, or another number from `1` to `21`. Text below that ratio is
  replaced with near-white or near-black, whichever contrasts more with its
  background; this is a threshold, not a guaranteed resulting ratio.
  Box/block, Powerline and sprite glyph colors are never adjusted.
  Changes apply immediately, including text already on screen.
- `cell_width`: `"font"` (default) keeps the terminal cell width unrounded;
  `"pixel"` puts it on whole device pixels after `letter_spacing`, rounded as
  `cell_rounding` says (JetBrains Mono at 12 on a Retina display: 14.4 to 14
  device pixels). Changes apply immediately.
- `app_icon`: the app's icon. `"classic"` (default), `"midnight"`, `"mirage"`, `"glass"`, `"phosphor"` or `"graphite"`, all drawn from Herdr's own artwork; `"theme"`, the same artwork in the colours of the theme showing (its background, foreground and accent), which changes with the theme, light and dark included; or an absolute path (or one starting `~/`, your home folder) to an `.icns`, `.png` or `.svg` file; a relative path is refused, like any other unknown value. On macOS the Dock shows it from start-up and changes as soon as settings.json does; a file that can't be read is tried again whenever settings change. An install from a checkout (`just install`) also writes it into the app bundle (an `.icns` or `.png`; an `.svg` stays in the Dock only) or, on Linux, the launcher (an `.svg` or `.png`), so Finder, Spotlight and the app switcher match. `"theme"` is drawn while Herdr runs, so it is in the Dock only: an install puts classic on the bundle. Release packages keep the classic icon. Settings > Appearance > Window has it as "App icon".
- `cell_rounding`: how the terminal grid snaps to whole device pixels: the row height (the font's ascent plus descent, times `line_height`) always, and the cell width when `cell_width` is `"pixel"`. `"outward"` (default) rounds the row up and the width down, so either can be up to a device pixel off the font's size. `"nearest"` rounds both to the nearest device pixel, at least one, so the grid is never more than half a pixel off the font's design, as Ghostty sizes its cells. A row rounded below the font's height keeps text centred, so tall glyphs reach past it equally above and below (at most a quarter of a device pixel each at `line_height` 1); program backgrounds paint before text and don't cover it, the cursor can. Changes apply immediately.
- `letter_spacing`: px (at zoom 1, like `font_size`) added to the terminal
  cell width, from `-2` to `10` (default `0`, the font's own advance).
  Negative values tighten the grid: JetBrains Mono at 12 is 7.2px wide, so
  `-0.2` gives 7px cells. The cell stays between half and one and a half of
  the font's advance. Scales with zoom. Changes apply immediately.
- `clipboard_write`: `false` blocks terminal programs from writing the local
  clipboard (default `true`, including remote terminals attached in this window).
  OSC 52 accepts base64 UTF-8 text up to 1 MiB: `c` writes the system clipboard,
  `p` writes Linux primary selection. `s` is ignored; cut buffers `0`–`7`
  are treated as the system clipboard.
  Clipboard reads (`?`) are never allowed.
- `pane_scrollbars`: `false` hides the scroll position thumb at a terminal
  pane's right edge (default `true`). It shows only when the pane has
  scrollback — never on the alternate screen (editors, pagers) — and
  drags to move through it.
- `focus_border_width` (px at zoom 1, `1` to `3`): the width of the active pane's border where the layout frames panes with a `focus` line; unset, the layout's own width. Every pane in the frame sits in by the wider of this and the `border` width, so the focus moving never resizes a terminal. A value outside `1` to `3` is reported as a problem and drawn at the nearer end. Its color is the theme color `focus` (set `colors.focus`), which a layout's frame draws where its `focus` line names it. The Settings window sets both on its Panes page, as "Focus border" (From theme, or one of the theme's colors as it is now) and "Focus border width" (Layout, 1, 2 or 3 px).
- `cursor_trail` (`"off"`, `"panes"` or `"all"`): `"panes"` draws a trail from one pane's cursor into the next whenever the focus moves between panes of the shown tab (keys, a click, `pane.go`, `agent.next`, float cycling, a split or close), in `cursor_trail_style`'s look, gone within `cursor_trail_duration_ms`. Each end is the terminal's cursor cell, shown or hidden (agents hide theirs at their input), else the pane's middle when the cursor is scrolled out of view; a pane that isn't a terminal gets none. `"all"` also draws one when the focused pane's cursor jumps, as kitty does: once it has been still for `cursor_trail_settle_ms` and has moved at least `cursor_trail_min_cells` from where it last settled, from there to here, so streaming output draws nothing until it stops; a hidden cursor draws none, and other panes' cursors never do. `"off"` (the default) draws nothing and costs nothing. The Settings window sets the cursor settings on its Cursor page (Appearance > Cursor): "Cursor trail" (Off, Between panes, Panes and jumps), "Trail style" while it's on, "Arrival effect", "Effect time" while either is on, and "Settle time" and "Jump distance" for Panes and jumps.
- `cursor_trail_style` (`"comet"`, `"tail"`, `"sweep"` or `"warp"`): how the trail looks. `"comet"` (the default): a glowing streak, its head about a cell high on the new cursor at once, tapering to a point that retracts into the cursor, as Rex draws. `"tail"`: the cursor stretched from the old place to the new, its corners catching up (those facing the move first), as kitty draws. `"sweep"`: a solid block from the old cursor shrinking into the new one. `"warp"`: a stretched block with soft edges, strongest at the cursor, as Neovide draws. The looks follow sahaj-b/ghostty-cursor-shaders.
- `cursor_arrival` (`"off"`, `"ripple"`, `"boom"`, `"ripple_rect"` or `"boom_rect"`): a pulse at the new cursor as the focus lands in another pane, with or without the trail (`cursor_trail` off draws the pulse alone), and with `cursor_trail: "all"` also after a jump or a change of the cursor's shape in the focused pane. `"ripple"` is an expanding ring, `"boom"` an expanding disc, `"ripple_rect"` and `"boom_rect"` the same as cursor-shaped boxes; each fades as it grows over `cursor_trail_duration_ms`. `"off"` (the default) draws none.
- `cursor_trail_duration_ms` (`50` to `1000`, default `120`): how long the trail and the arrival pulse take; the trail's tail decays into the cursor in about a third of it. A value outside the range is reported as a problem and used at the nearer end.
- `cursor_trail_settle_ms` (`0` to `500`, default `80`) and `cursor_trail_min_cells` (`1` to `50`, default `2`): with `cursor_trail: "all"`, how long the cursor must stay still before a jump counts, and how many cells (the larger of columns and rows, so a diagonal step is one) it must have moved from where it last settled. Values outside the ranges are reported as problems.
- `cursor_trail_color` (`"#rrggbb"`): the color of the trail and the arrival pulse (the comet's core paler); unset (the default), the theme's `cursor`. Anything else is reported as a problem and the cursor color is used. The Settings window doesn't set it.
- `telemetry`: `false` stops anonymous usage data and deletes what wasn't
  sent (on when absent; Settings → Privacy shows each report word for
  word, and `telemetry()` in mods). Once a day the app sends counts:
  version, OS, which known agents ran, how many spaces, tabs and
  terminals, which layouts, shipped mods and commands were used. Never
  terminal content, names of files, folders, spaces or the person's own
  mods; nothing from herdr-core. `DO_NOT_TRACK=1` or `HERDR_TELEMETRY=0`
  turns it off for one run. A build from source never sends.

## Programs: `programs.json`

Every pane, agent and terminal carries `program`: its name, icon and tile,
from one table, so every mod shows the same, and the app's own toasts
("Kiro finished") use the same names. An agent's own report wins;
else the program the terminal runs (an interpreter shows what it runs:
`node .../vite` is Vite, `npx vitest` is Vitest); shells are `shell`,
anything unlisted is `program` with a color of its own. The built-in table
has 120 programs (agents, editors, runtimes, tools, databases, infra);
`programs.json` here adds to it or changes entries field by field:

```json
{ "programs": {
    "lazygit": { "name": "Git", "tile": "#f05032" },
    "mytool": { "name": "My Tool", "icon": "my-icons/mytool.svg", "tint": true,
                "tile": "#334155", "color": "#ffffff", "match": ["mytool-dev"] } },
  "shells": ["xonsh"], "interpreters": ["pipx"] }
```

- `icon`: a path in the home folder. `tint: true` for a one-color glyph
  (drawn in `color` on a tile, or the text color), false for a picture.
- `match`: other process names (or agents' `app` names) for the entry.
- `run`: how to start it (`["cursor-agent"]`), for agents people start
  from a menu: `knownAgents()` lists the entries with one, and whether this
  machine (or another) has them installed. For example, override
  `"claude": { "run": ["my-claude"] }` or add
  `"claude-alt": { "name": "Claude (alt)", "icon": "icons/claude.svg", "run": ["my-claude-alt"] }`
  inside `programs`. Use programs or shell functions, not aliases (the launcher
  does not expand them). On macOS the menu launcher loads login profiles before
  running the agent and leaves an interactive login shell when it ends.
  Codex starts as `codex --no-daemon`: a shared Codex daemon would make later
  sessions use the first terminal's identity and stop with it.
  On remote Unix machines, both the installed-agent check and launcher load
  that user's login/interactive shell for sh, bash, zsh, dash, ksh and fish, so
  its PATH includes shell-configured tools. Other shells keep the sh launcher.
  Local Linux launches keep interactive non-login shells. Local and remote checks
  include functions in bash and zsh; fish checks find executables only. Installation
  checks are cached for up to a minute; changing the commands checks again.
  Invalid JSON is reported in `programs.error` (a valid file has no error).
- It follows what runs in the terminal now (lazygit, nvim), else what it
  started with.
- `resume`: how to bring the program back after a reboot, when herdr-core gives its kept terminal a fresh shell. While it runs, Herdr records the command line that resumes it in `data/resume.json` here; `session.resume` (and the toast `resume_agents` asks with) types it back in. Claude Code ships with one:

  ```json
  "claude": { "resume": {
      "session": { "file": "~/.claude/sessions/{pid}.json", "field": "sessionId", "when": { "kind": "interactive" } },
      "drop": { "--resume": 1, "-r": 1, "--session-id": 1, "--continue": 0, "-c": 0, "--fork-session": 0, "--from-pr": 1 },
      "append": ["--resume", "{session_id}"] } }
  ```

  `drop` takes flags out of the command line it ran with (the number is
  how many values each takes at most), the rest (model, permissions)
  stays, then `append` is added, `{session_id}` being its session. The
  session comes from the agent's own report, the terminal label
  `herdr.session.<program key>=<id>` (or `<id>@<pid>`, honored only for
  that process; an empty value clears it), which a hook sets with
  `hc label "$HC_TERMINAL_ID" herdr.session.claude=<id>`, else from
  `session.file` (`{pid}`: the program's process id; `~/`: the home of
  the machine it runs on), whose `field` is the
  id and whose `when` fields must all match. An id is at most 256 letters,
  digits or `_ . / : = + -` and never starts with `-`, and an `append`
  word uses only those characters; others are refused (a rule with a bad
  word is dropped and named in `programs.error`).
  `{ "rerun": true }` runs the same command line again; `"resume": {}`
  turns a shipped rule off. Claude Code writes its session file once, at
  start, so after `/clear` or `/resume` only a hook's label is current.
  Other agents need a rule with `append` (e.g.
  `"codex": { "resume": { "append": ["resume", "{session_id}"] } }`) and a
  hook that reports the id.
- `session_name`: where the program's session keeps its own name, read for `session` and `label` on panes (see Panes): `{ "file", "field", "when"? }` as in `resume.session`, the file read only for terminals on this computer, once per process and again when its modification time moves. Claude Code ships with `"session_name": { "file": "~/.claude/sessions/{pid}.json", "field": "name" }`; a name needs a letter or digit.
- A record goes when its program ends in a terminal that kept running (the person quit it), when its terminal goes while Herdr watches, or when another session replaces it; a reboot or core restart keeps it.

## Mods: `mods/<name>/`

```
~/.config/herdr-app/mods/<name>/
  main.js       the mod (required)
  mod.json    where it first appears: { "place": "right" | "left" | "top" | "bottom" | "none", "size": 320 }, palette "actions", terminal "menu_items"
  *.js, *.svg   more modules ("./ui.js"), its own icons
```

- `"actions"` in `mod.json` adds entries to the command palette, listed under the mod's name: `[{ "title": "Send to all terminals", "command": "mod.open", "args": { "props": { "target": "shells" } }, "keywords": "broadcast" }]`. `command` is an app command (see Commands) run with `args` (optional), or one of the mod's own `<mod>.<command>` (no `args`), delivered to its `nextCommand()` as a key would, while a copy is shown (Herdr can't check the command's name; `mod.clone` renames them); `keywords` (optional) are more words to find it by. `mod.open` without a `name` opens this mod, so an action can open the mod's own overlay with props that say what it's for. An entry the app can't run (no title, an unknown command, a misspelled or missing argument) is left out and named in `herdr-app status` and `herdr-app check`, as is a mod.json Herdr can't read; the mod's other entries and fields still count. Saving `mod.json` updates the palette, and a removed mod's actions go with it. `modActions()` lists every mod's actions as the palette shows them.
- `"menu_items"` in `mod.json` adds entries to the terminal right-click menu (the shipped `menu`'s pane menu, also what `pane.menu` opens), after Herdr's own and before Close pane, one group per mod under a divider: `[{ "title": "Move to…", "command": "mod.open", "when": "pane", "icon": "move.svg" }]`. They act on the terminal right-clicked, not the focused one: an argument `pane` or `terminal` the command takes and the entry leaves out is filled with the clicked target's, and `mod.open` gets `props.target` (`{pane, terminal, selected, cwd, last_output}`, as a terminal menu's `props().target`, replacing a `target` of the entry's own props), so `mod.open` without a `name` opens this mod for that pane. `when` (optional) shows it only if the terminal had a selection (`"selected"`), its folder is known (`"cwd"`) or it is a tab's pane (`"pane"`, not a layout terminal block); an entry filling `pane` needs one too. `icon` (optional) is an `.svg` in the mod's folder; otherwise the command's. Entries are checked as `actions` are and named in `herdr-app status` and `herdr-app check`. `menuItems()` lists them with `mod`, `args`, `when`, `icon` (a path for `svg()`) and `fill` (the arguments to fill: `pane`, `terminal`, `props`), so a layout's own `terminal_menu` can show them; nothing is added to it otherwise.
- `"focusable": true` in `mod.json` lets `pane.go` give a shown copy the keyboard.
  It gets `"focus"` from `nextCommand()`: focus your own handle then
  (`this.handle.focus()`) and handle keys with `on_key_down`. The copy gets `"focus"`
  again after a popup over it closes. Leave with `run("pane.go", {direction})`.
  Without this opt-in, navigation and clicks stay unchanged.

- The first time Herdr sees a folder, it docks one copy per `mod.json` into
  the shown layout (`"none"`: only where a layout or another mod puts it).
  After that, the layouts decide. Saving any file reloads every copy in place
  (not files the app writes: `jsconfig.json`, `.d.ts`, hidden ones,
  `node_modules`). The old build's tasks and timers stop: a loop ends at its
  next `cx` use. A broken save keeps the old version running and shows in
  `herdr-app status`; its top-level code has already run, so keep side effects
  on `globalThis` out of the module's top level.
- A copy starts the first time it shows, then stays alive while its block is in
  the layout, shown or not (a hidden column, the other child of a stack, a pane
  in another tab or behind a zoom): switching back finds it as it was. Its
  timers keep running while hidden, so poll gently. Another layout starts its
  mods fresh (pane mods belong to their tab and stay); a block removed or given
  another mod or props starts over. `<mod>.<command>` reaches shown copies only.
- Images and SVGs are referenced from `~/.config/herdr-app`:
  `image("mods/<name>/logo.svg")` (full color), `svg("mods/<name>/icon.svg")`
  (tinted with the text color). Saving an asset under `mods/` updates every
  mod showing it, including images that were missing or invalid on first load.
- The defaults are mods too, read them as examples: `spaces`, `agents`,
  `tabs` (the tab bar), `menu`, `prompt`, `confirm`, `help`, `toast`,
  Terminals' `picker`, `peek`, and `pane` (an
  example pane header, not used by the default layouts), and `worktree`
  (git worktrees as spaces: the space menu's and the + menu's New worktree,
  Open worktree (the repo's checkouts, filtered; one already shown is gone
  to) and Remove worktree open it as a dialog with props `{space, folder?,
  action: "new" | "open" | "remove"}` (the repo: folder, else where the
  space's first terminal is), or with `{branch, tab?, argv?}` to make one at once with
  that first tab; checkouts go to
  `~/.herdr/worktrees/<repo>/<branch>` on the space's machine, a constant
  at its top; the
  `spaces` sidebar nests worktree spaces under their repo's and shows each
  space's repo, branch, ahead/behind and uncommitted changes). Shipped mods
  are Herdr's; duplicate with `mod.clone` before editing.

### Check before you finish

```sh
herdr-app check ~/.config/herdr-app/mods/<name>
```

Loads the mod, renders it once, exits nonzero with the exact error. Run it
after every change. Never call a mod done without a passing check.

### A mod

```js
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, spaces, theme, run } from "herdr";

export default class Branches extends View {
  init(_props, cx) {
    this.branch = {};
    cx.timer.every(3000, (cx) => this.refresh(cx));
    this.refresh(cx);
  }

  refresh(cx) {
    cx.spawn(async (cx) => {
      for (const p of spaces()) {
        // on: a space on another machine runs git there.
        const r = await exec("git", ["branch", "--show-current"], { cwd: p.folder, on: p.machine });
        this.branch[p.id] = r.stdout.trim();
      }
      cx.notify();
    });
  }

  render(cx) {
    const t = theme();
    return v_flex().size_full().bg(t.surface).p(10).gap(4).text_size(13).children(
      spaces().map((p) =>
        h_flex().id(`b-${p.id}`).gap(8).hover((s) => s.bg(t.raised))
          .on_click(() => run("space.select", { id: p.id }))
          .child(div().text_color(p.active ? t.foreground : t.muted).child(p.name))
          .child(div().text_color(t.dim).child(this.branch[p.id] ?? ""))));
  }
}
```

Rules:
- `main.js` default-exports a class extending `View`. `init(props, cx)` runs
  once; `render(cx)` describes the UI and runs again after `cx.notify()` and
  whenever Herdr's live data changes (spaces, agents, terminals, focus,
  theme, shared state). Don't poll those.
- Module code runs once per source version and is shared by copies, even after all
  copies close. Keep instance state and host calls in constructor/init, not module
  scope. Each copy still gets its own instance, tasks and localStorage.
- **Never keep a `cx` and use it later.** Every callback gets its own:
  `on_click((event, cx) => …)`, `cx.spawn(async (cx) => …)`,
  `cx.timer.every(ms, (cx) => …)`.
- No `setTimeout`/`setInterval` (use `cx.timer`), no top-level `await` (use
  `cx.spawn` in `init`). Anything slow goes through `exec` or `fetch`.
- Size: put `.id("root").on_size_change((size, cx) => { this.size = size; cx.notify(); })`
  on the root `div()`, `h_flex()` or `v_flex()`. It reports `{width, height}`
  in logical pixels after the first layout and whenever the size changes,
  even with the pointer elsewhere. It does not change `props()`.
- Cleanup: an optional `async close(cx)` on the exported `View` gets one final
  call when its pane, tab, layout-owned slot or overlay goes, and on app quit.
  It is not called for a hidden copy or an inactive tab. Start cleanup inside
  `close()`; don't await work started earlier. Await cleanup directly,
  for example `async close(cx) { await exec("hc", ["kill", this.browser]); }`.
  Closing stays responsive: cleanup has at most 250 ms (shared across copies
  on quit), then unfinished tasks are cancelled. On quit, use `exec(...)` and
  `run("terminal.kill", { terminal: address })`: those go out before the event
  loop stops; other commands may not run. An `exec` already started keeps
  running; don't start new long-lived work in `close`. This is best-effort,
  not a guarantee after a crash or forced termination.
- Use `theme()` colors so the mod follows the user's theme.
- **Animate natively, never with a timer.** Every redraw of a mod redraws
  the whole window, so `cx.timer.every(90, () => cx.notify())` for a
  spinner costs real battery. Use `.frames(["⠋", "⠙", …], 90)` (shows
  each string in turn as the element's text, in a box as wide as the widest, grayscale), `svg(arc).spin(900)` (turns
  an icon once every 900 ms: a ring spinner, e.g.
  `mods/workspace-chrome/spinner.svg`), or `.loop("opacity", { from:
  0.4, to: 1, period: 1200 })` (also width, height, left, top), and
  `.transition(...)` for changes: they run without your script and stop
  when not shown. Timers are for data (a clock, a fetch), and only as often
  as what they show changes.
- **An agent at work shows the person's spinner.** Read `settings().working_spinner` (see `working_spinner` under The person) rather than picking your own, so every mod spins the same way.

### UI

Chained builders with GPUI's snake_case names: `div()`, `h_flex()`,
`v_flex()`; `.size_full()`, `.flex_1()`, `.gap(n)`, `.p(n)`, `.px(n)`,
`.w(n)`, `.h(n)`, `.items_center()`, `.justify_between()`,
`.overflow_y_scrollbar()`; `.child("text")`, `.text_size(n)`, `.font_semibold()`,
`.truncate()`, `.text_color(c)`; `.bg(c)`, `.border(1)`, `.border_color(c)`,
`.rounded(n)`; `.id("unique")` then `.on_click(...)`, `.on_mouse_down("right", ...)`,
`.hover((s) => s.bg(c))`, `.on_scroll_wheel(...)`; keyboard: `this.h = cx.focus_handle()`
in init, `.track_focus(this.h).on_key_down((e, cx) => …)`, `this.h.focus()`.
Tab and Shift-Tab never reach `on_key_down` (they move focus): bind them in the mod's keys.json section (Keys).
`.on_drop((event, cx) => …)` on a plain layout element receives external files as
`event.paths` (absolute paths). The feedback form accepts dropped images with
the same limits, previews and removal as picked images.
`.on_hover((hovered, cx) => …)` reports pointer entry and exit on buttons
and other interactive controls too.
`.when(cond, (el) => …)`, `.children([...])`. Controls from `gpui-base`
(`Button`, `Checkbox`, `Switch`, `InputState` + `Input`) are unstyled: style
them. Text: `TextView.markdown(id, text)` renders markdown; `EditorState.new({
language, value, readonly, font_family: settings().font_family, syntax: {
keyword: theme().ansi5, ... } })` + `Editor.new(state)` is a code view (readonly)
or editor with syntax highlighting, line numbers and folding. A viewer opens
beside the shell as a pane: `pane.split {direction, mod, props: { file }}`.
The full surface is in `gpui-kit.d.ts` next to this file.

### The `herdr` module

All of it is in `herdr.d.ts`. In short:

- Live data: `spaces()` (each with its tabs, status, name, named, folder,
  machine, branch), `machines()` (name, null for this computer; layout:
  `loading`, `ready`, or `unavailable` on this connection; loading ends on
  the first read even if it fails, and offline machines are unavailable),
  `agents()` (status, app, space, tab, label, session, pinned, focused, card_info; "done" turns "idle"
  once someone looks at it on any Herdr client), `terminals()`
  (every core terminal), `focused()`, `layout()`, `theme()` (every color by
  name: background (the terminals', cards), canvas (behind cards), chrome
  (sidebars, bars), surface, popover, raised, hover, selected, border,
  input (a stronger border), foreground, muted, dim, accent,
  accent_foreground, solid and solid_foreground (a primary button), ring
  (focus rings), focus (the active pane's border), selection, working,
  blocked, done, idle, unknown, ansi0..15, and the
  toast's own: toast, toast_border, toast_shadow, toast_foreground,
  toast_muted, toast_info, toast_success, toast_warning, toast_error; light
  and dark themes name the same ones), `settings()` (with `dark`: the
  appearance now), `window()` (with `scale_factor`: device pixels per px
  of the screen the window is on, 1 or 2, changing live as it moves to
  another screen; round sizes to whole device pixels with it), `keys()`,
  `commands()` (every app
  command, its arguments and what it does), `modActions()` (every mod's palette
  actions: mod, title, command, args, keywords), `menuItems()` (every mod's
  terminal menu items: mod, title, command, args, when, icon, fill),
  `layouts()` (the layout names).
  `focused().agent` is the agent an agent block shows.
  `focused().space`/`tab` describe the current window context. With a layout terminal
  block focused, `pane` and `shown` are null; `terminal` and `cwd` describe that block,
  not its source pane. A terminal in a window (`openOverlay` with a `layout`) that has
  the keyboard is reported the same way; pane commands without a `pane` still act on
  the focused pane under the window. Check `focused()?.pane` before offering pane controls.
  `panes()` still lists only model panes; none is focused while a terminal block is.
  For composition navigation: `run("pane.focus", {terminal: nextAddress})`.
  `recentPlaces()`: where the person has been, newest first (pane, terminal, tab,
  space, machine, at), one place per pane, at most 20. A place counts once they
  stay on it 300 ms, by any means (keys, clicks, commands, `agent.focus`, tab and
  space switches) on any Herdr client of this machine; overlays and popups never
  count, so a go-to popup sees where the person was before it opened. A place is a
  pane: `agent.focus` on a sub-agent shown only in an agent block records none. Closed panes
  stay in it until pushed out: check them against `panes()`. Kept across restarts
  in the core document `herdr.focus` (see herdr-core below).
  Terminal copy/paste and implicit right-click routing follow the focused block.
  Reads subscribe: a copy re-renders only when something it read
  changes, so read what you show and no more. Each call returns a fresh
  copy of the whole list: read it once per render into a variable, not
  once per row. A hidden copy (another tab's pane header, an inactive
  stack child) is not redrawn until shown.
- Git: `gitStatus(folder, { on, files })`: the status of the repo `folder` is in (branch, head, upstream, ahead, behind, staged, unstaged, untracked, conflicted, root, name, linked; with `files: true`, each changed file's path and git's two status letters), `null` until the first answer, `false` outside a repo. Cached and shared by every mod: git runs once per change on disk (ignored folders don't count), never once per mod or per render, and the copy re-renders when that folder's answer changes. Read it in render instead of running `git status` on a timer. `nextGitStatus(folder, { on, files, after })` resolves with `{ status, revision }` once there is an answer newer than `after` (for work outside render); `refreshGitStatus(folder, { on })` asks again now. Another machine's folder (`on`) has no watcher: refresh it when a command ends there; it is also asked again when read a minute old. To keep a fallback for builds without them, `import * as herdr from "herdr"` and check `typeof herdr.gitStatus === "function"`.
- Floating: `placement()` (null unless this copy floats).
- Act: `run(command, args)`.
- Overlays: `openOverlay({ mod, props, kind: "dialog" | "popup" | "window", width, height, at, frame })`
  returns an id; the mod gets `props().overlay` and closes itself with
  `closeOverlay(id)`; Esc closes the top dialog or popup. `frame: "none"`
  drops Herdr's border, corners and shadow so the mod draws its own.
  `notify({ title, body, level, icon, timeout })` posts a notification, drawn by the `toast` mod
  in an overlay as wide as its content (200 to 340 px) and 40 px high, 72 with a body (blank text is none).
  It goes after `timeout` ms when given (the person's `toast_duration` doesn't change it), else after
  5 s (8 s for errors and warnings) scaled by `toast_duration`, or not until clicked. A replacement
  `toast` mod finds the level mark to draw in `props().toast_mark` (the
  person's, as the design takes it: a pill's stripe is a dot, the outline,
  terminal and icon card draw none)
  and their design in `props().toast_design` (with `toast_font` for
  `"terminal"` and `toast_more` on the banner shown); Herdr sizes, frames
  and places each design, and draws `toast_progress`'s bar.
  At most 5 toasts show: past that the oldest close, routine ones before
  errors and warnings, never the one just posted.
- Windows (`kind: "window"`): stay open until closed, over the layout and
  under dialogs; a press the mod doesn't stop drags it (stop it on your
  controls, like a title bar's buttons), it stays where dropped, and the one
  pressed last is on top. Sized by content unless `width`/`height`; at `at`
  or centered. `key: "name"` remembers its spot (opening that key again
  replaces it); `keep: true` with a key reopens it on the next start. A
  layout switch closes windows; kept ones come back with their layout.
  A window can hold a layout, terminals too: a terminal keeps its presses
  (selecting text), so give it a mod block to drag by, a title bar:
  `openOverlay({ kind: "window", key: "t7", width: 600, height: 400, layout: { type: "column", children: [{ type: "block", size: { px: 28 }, content: { mod: "my-titlebar" } }, { type: "block", content: { terminal: "t7" } }] } })`.
- Machine: `exec(cmd, args, { cwd, on, timeout })` (the user's real environment; use this,
  not `process.run`; `on: space.machine` runs there, omitted means local).
  `timeout` is milliseconds, defaults to 30,000 and is capped at 600,000 (10 minutes),
  locally and remotely. It rejects on timeout, if the program cannot start or the machine cannot be reached.
  `hc(args)` (`hc read <t>` gives a terminal's text),
  `terminalLines(terminal, { lines })` (the screen's last lines that carry anything, from the app's own grid, as `{ text, faint }`: `faint` lists the `[start, end)` `text.slice` stretches drawn faint, SGR 2, such as Claude Code's suggested next prompt; null when no tab here holds it),
  `home()`, `appHome()`, `playSound("done" | "blocked" | file)`.
  `knownAgents({ on })`: the agents people can start (programs.json's
  `run`), most picked first, each `installed` or not there (asked of the
  shell agents start in; a remote Unix machine's login/interactive shell when supported);
  start one with `argv: agent.run`.
  `pickFolder({ prompt })`: the system's folder picker (here only),
  resolves with the folder or null.
- Files: `readTextFile(path, { on })` (here, or on that machine),
  `writeTextFile(path, text, expected)` (here only), and
  `watchFolder(dir)` (here only; poll a remote space's folder): resolves with the changed paths the next time anything
  under `dir` changes. On Linux, Git-ignored directories and Git objects are skipped,
  and trees above 4096 non-ignored directories reject the watch; use a timer fallback.
  macOS and Windows watch recursively, including ignored directories.
  A returned root path means refresh everything (the pending paths are bounded).
  Files also refreshes on its 10-second timer:
  `cx.spawn(async (cx) => { for (;;) { await watchFolder(dir); this.reload(); cx.notify(); } })`.
- `notify({ ..., action: { name, args } })`: the toast runs the command when clicked.
- This copy: `props()`, `shared(key)` / `setShared(key, value)` (every mod
  sees it; setting re-renders the copies that read that key), `nextCommand()` (key commands for this
  mod: `cx.spawn(async (cx) => { for (;;) { const c = await nextCommand(); …; cx.notify(); } })`).

Mods are trusted code: commands and network access (including `fetch`) are unrestricted.
Other built-ins: `localStorage` (per mod copy), `console.log`, `fetch`,
`import * as fs from "fs/promises"` (read in home, write under `~/.config/herdr-app`).

## herdr-core (`hc`)

`herdr-app --skill` prints the manual for driving this window;
`hc --skill` prints the manual for running and coordinating programs and agents,
including a small herdr-core skill you can install with the user's permission.
`herdr-app commands` lists app commands and arguments without needing an open window.

- `hc ls --json`, `hc read t3 --lines 50`, `hc run --json --cwd <dir> --label k=v -- <cmd>`,
  `hc label t3 k=v`, `hc send t3 "text"`, `hc wait t3`. `hc --help` for all.
- Other machines: `hc machine ls`; `hc run --on <machine> ...` starts a
  terminal there (`vps/t7`), `hc exec --on <machine> git status` runs one
  command there and prints exactly its output.
- Focus history: the core document `herdr.focus`, read and written with
  `doc.get`/`doc.set` through the API (application programming interface)
  only, is `{"v": 1, "places": [{pane, terminal, tab, space, machine, at},
  ...]}`, newest first, at most 20, one place per pane; `pane` (a
  non-empty string) and `at` (an integer, ms since the Unix epoch) are
  required. Herdr app windows write it on this machine's core when the
  person settles on a pane. Any client may add a visit: read, merge (the
  union by `pane`, the later `at` winning, sorted by `at` newest first
  then `pane`, cut to 20), and `doc.set` with the version read; on
  `conflict`, read and merge again. Write only when the merge adds a
  visit, and keep fields and entries you don't read. A stored `v` above 1
  is a newer shape: read it, never write it. A value that isn't an object
  with an integer `v` is unreadable, and the app's next write replaces it.
  `{"v": 1, "places": []}` clears it.

## Keep in mind

- A task's `cx` belongs to the copy that started it: `cx.notify()` redraws
  that copy, even when another copy woke it (a shared promise).
- One mod per idea. Don't edit `AGENTS.md` or the `.d.ts` files (the app
  rewrites them). Don't touch other mods unless the request
  needs it.
