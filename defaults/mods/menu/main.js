// Context menus (Herdr's items). Opened as a popup with
// props { tab: "pane" | "tab" | "space" | "global", target }.
// ↑/↓ move, Enter runs, Esc closes. An item is [label, command, args, from?];
// a command "worktree" opens the worktree dialog instead, "icon" the icon picker.
// `from` is a mod's entry from menuItems(), for the pane menu's mod group.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { run, props, closeOverlay, openOverlay, theme, keys, terminals, spaces, panes, exec, menuItems } from "herdr";

// Where a space is now: its first terminal's live folder, else where it was made.
function place(s) {
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return panes().find((p) => tabs.has(p.tab) && p.cwd)?.cwd || s.folder;
}

// A space's checkout: "main" (a repo's own), "linked" (a worktree), or null.
async function checkout(folder, on) {
  const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd: folder, on });
  const common = await exec("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], { cwd: folder, on });
  if (top.code !== 0 || common.code !== 0) return null;
  return common.stdout.trim() === `${top.stdout.trim()}/.git` ? "main" : "linked";
}

// Whether a mod's menu item shows for this right-click: its `when`, and a
// target for every argument it fills (a terminal outside a tab has no pane).
function shows(item, tg, inTab) {
  const fills = (k) => item.fill.includes(k);
  if (item.when === "selected" && !tg.selected) return false;
  if (item.when === "cwd" && !tg.cwd) return false;
  if ((item.when === "pane" || fills("pane")) && !inTab) return false;
  return !fills("terminal") || Boolean(tg.terminal);
}

// A mod's item's args with the right-clicked target filled in: the arguments
// it leaves open, and mod.open's props.target (replacing one the entry gave:
// the clicked terminal is what the item acts on).
function targeted(item, tg) {
  const args = { ...item.args };
  for (const k of item.fill) {
    if (k === "props") args.props = { ...args.props, target: tg };
    else args[k] = tg[k];
  }
  return args;
}

// Copy, copy last output, copy all, paste, copy path: a group of their own at the top of the pane menu.
const clip = (command) => command.startsWith("terminal.copy") || command === "terminal.paste";

function items(p, git) {
  p = p ?? {};
  const tg = p.target ?? {};
  switch (p.tab) {
    case "pane": {
      // A kept sub-agent can go back under its parent, still running.
      const all = terminals();
      const term = all.find((t) => t.address === tg.terminal);
      const parent = term?.parent && all.find((t) => t.address === term.parent);
      // Once they go to the program, Shift+right-click still opens this menu.
      const toProgram = term?.labels?.["herdr.right-click"] === "program";
      // A terminal shown outside a tab (a layout terminal block, a window) has
      // no pane to split, zoom or close: its pane is the block's id.
      const inTab = panes().some((pane) => pane.pane === tg.pane);
      // The window says whether the clicked terminal had a selection and where it is.
      const on = term && { terminal: term.address };
      const info = inTab && panes().find((pane) => pane.pane === tg.pane);
      const own = info?.own_icon;
      const floating = info?.floating;
      return [
        ...(term
          ? [
              ...(tg.selected ? [["Copy", "terminal.copy", on]] : []),
              // The window says whether the shell marked a finished command's output (OSC 133).
              ...(tg.last_output ? [["Copy last output", "terminal.copy_last_output", on]] : []),
              ["Copy all", "terminal.copy_all", on],
              ["Paste", "terminal.paste", on],
              ...(tg.cwd ? [["Copy path", "terminal.copy_path", on]] : []),
            ]
          : []),
        ...(inTab
          ? [
              ["Rename", "pane.rename", { pane: tg.pane }],
              ["Split right", "pane.split", { direction: "right", pane: tg.pane }],
              ["Split down", "pane.split", { direction: "down", pane: tg.pane }],
              ["Zoom", "pane.zoom", { pane: tg.pane }],
              ["Change icon…", "icon", { pane: tg.pane }],
              ...(own ? [["Clear icon", "pane.set_icon", { pane: tg.pane, icon: null }]] : []),
              floating ? ["Dock pane", "pane.dock", { pane: tg.pane }] : ["Float pane", "pane.float", { pane: tg.pane }],
              ...(floating ? [["Bring to front", "pane.raise", { pane: tg.pane, to: "front" }]] : []),
            ]
          : []),
        ...(term
          ? [
              toProgram
                ? ["Use Herdr right-click menu", "terminal.right_click", { terminal: term.address, to: "menu" }]
                : ["Send right-clicks to pane", "terminal.right_click", { terminal: term.address, to: "program" }],
            ]
          : []),
        ...(parent ? [[`Send back to ${parent.name || parent.address}`, "terminal.park", { terminal: term.address }]] : []),
        ...menuItems().filter((it) => shows(it, tg, inTab)).map((it) => [it.title, it.command, targeted(it, tg), it]),
        ...(inTab ? [["Close pane", "pane.close", { pane: tg.pane }]] : []),
      ];
    }
    case "tab": {
      const tabs = spaces().find((s) => (s.tabs ?? []).some((c) => c.id === tg.id))?.tabs ?? [];
      const at = tabs.findIndex((c) => c.id === tg.id), tab = tabs[at];
      return [
        ["Rename", "tab.rename", { id: tg.id }],
        ...(at > 0 ? [["Move left", "tab.move_left", { id: tg.id }]] : []),
        ...(at !== -1 && at < tabs.length - 1 ? [["Move right", "tab.move_right", { id: tg.id }]] : []),
        ["Change icon…", "icon", { tab_id: tg.id }],
        ...(tab?.own_icon ? [["Clear icon", "tab.set_icon", { id: tg.id, icon: null }]] : []),
        ["New tab", "tab.new", { space: tg.space }],
        ["Close", "tab.close", { id: tg.id }],
      ];
    }
    case "space":
      return [
        ["Rename", "space.rename", { id: tg.id }],
        ...(git ? [["New worktree", "worktree", { space: tg.id, action: "new" }], ["Open worktree", "worktree", { space: tg.id, action: "open" }]] : []),
        ...(git === "linked" ? [["Remove worktree", "worktree", { space: tg.id, action: "remove" }]] : []),
        ["Close", "space.close", { id: tg.id }],
      ];
    default:
      return [
        ["New space", "space.new", {}],
        ["Keybinds", "app.help", {}],
        ["Theme: graphite", "app.theme", { name: "graphite" }],
        ["Theme: ink", "app.theme", { name: "ink" }],
        ["Theme: moss", "app.theme", { name: "moss" }],
        ["Theme: atom one dark", "app.theme", { name: "atom-one-dark" }],
      ];
  }
}

// Rows and rules, px: an item is ROW tall, and a rule above it RULE (1 px and its margins).
const ROW = 31, RULE = 9, PAD = 10;

// Whether item i starts a group, drawn with a rule above it: Herdr's clip items,
// Close, Keybinds and themes, or a mod's items (one group per mod, none inside).
function startsGroup(list, i) {
  if (i === 0) return false;
  const it = list[i], before = list[i - 1];
  if (it[3]) return it[3].mod !== before[3]?.mod;
  return (clip(before[1]) && !clip(it[1])) || it[1].endsWith(".close") || it[1] === "app.help" || (it[1] === "app.theme" && before[1] !== "app.theme");
}

const itemHeight = (list, i) => ROW + (startsGroup(list, i) ? RULE : 0);

function menuIcon(command) {
  if (command === "icon") return "mods/icon-picker/smile.svg";
  if (command.endsWith(".set_icon")) return "mods/workspace-chrome/x.svg";
  const name = command === "terminal.copy" ? "copy" : command === "terminal.copy_all" ? "files" : command === "terminal.copy_last_output" ? "file-text" : command === "terminal.copy_path" ? "folder" : command === "terminal.paste" ? "clipboard" : command.endsWith(".close") ? "x" : command.endsWith(".rename") ? "square-pen" : command.endsWith(".new") ? "plus" : command === "worktree" ? "git-branch" : command === "app.help" ? "terminal" : command === "app.theme" ? "settings" : command === "pane.split" ? "panel-right" : command === "pane.zoom" ? "square" : command === "tab.move_left" ? "chevron-left" : command === "tab.move_right" ? "chevron-right" : "terminal";
  return `mods/workspace-chrome/${name}.svg`;
}

// The key bound to a command, for the hint on the right.
function shortcut(name, args) {
  // `pane.split:right` binds pane.split with { direction: "right" }.
  const value = args && Object.values(args)[0];
  const k = keys().find((b) => b.section === "global" && (b.action === name || b.action === `${name}:${value}`));
  return k ? k.keys : "";
}

export default class Menu extends View {
  init(_props, cx) {
    this.at = 0;
    this.scroll = 0;
    this.follow = false;
    this.git = null;
    this.handle = cx.focus_handle();
    this.handle.focus();
    const p = props();
    const space = p?.tab === "space" && spaces().find((s) => s.id === p.target?.id);
    const folder = space && place(space);
    if (folder)
      cx.spawn(async (cx) => {
        this.git = await checkout(folder, space.machine).catch(() => null);
        cx.notify();
      });
  }

  choose(i, cx) {
    const it = items(props(), this.git)[i];
    if (!it) return;
    closeOverlay(props()?.overlay);
    if (it[1] === "icon") openOverlay({ mod: "icon-picker", kind: "dialog", width: 452, height: 392, props: it[2] });
    else if (it[1] === "worktree") openOverlay({ mod: "worktree", kind: "dialog", ...(it[2].action === "open" ? { width: 560, height: 480 } : { width: 440, height: 260 }), props: it[2] });
    else run(it[1], it[2]);
  }

  // One item, under a rule when it starts a group.
  row(list, i, t) {
    const it = list[i];
    return v_flex()
      .when(startsGroup(list, i), (el) => el.child(div().mx(7).my(4).h(1).bg(t.border)))
      .child(h_flex()
      .id(`mi-${i}`)
      .h(ROW)
      .px(8)
      .gap(9)
      .rounded(7)
      .items_center()
      .justify_between()
      .text_color(t.foreground)
      .when(i === this.at, (el) => el.bg(t.selected))
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => this.choose(i, cx))
      .child(svg(it[3]?.icon ?? menuIcon(it[1])).size(14).flex_none().text_color(t.muted))
      .child(div().flex_1().child(it[0]))
      .child(div().text_size(11).text_color(t.dim).child(shortcut(it[1], it[2]))));
  }

  // The items above a trailing Close, scrolled when they don't fit in the
  // room inside the popup (props.height): Close stays pinned below them, and the
  // keyboard's selection is scrolled into view.
  body(list, t) {
    const pin = list.length > 1 && list[list.length - 1][1].endsWith(".close") ? 1 : 0;
    const n = list.length - pin;
    const tops = [];
    let height = 0;
    for (let i = 0; i < list.length; i++) {
      tops.push(height);
      height += itemHeight(list, i);
    }
    const room = (props()?.height ?? Infinity) - PAD - (pin ? itemHeight(list, n) : 0);
    const rows = Array.from({ length: n }, (_, i) => this.row(list, i, t));
    const pinned = pin ? [this.row(list, n, t)] : [];
    const listHeight = tops[n] ?? height;
    if (listHeight <= room) return [...rows, ...pinned];
    const most = listHeight - room;
    if (this.follow && this.at < n) {
      const bottom = tops[this.at] + itemHeight(list, this.at);
      if (bottom - ROW < this.scroll) this.scroll = bottom - ROW;
      if (bottom > this.scroll + room) this.scroll = bottom - room;
    }
    this.follow = false;
    this.scroll = Math.max(0, Math.min(this.scroll, most));
    return [
      div().id("menu-body").relative().h(room).flex_none().overflow_hidden()
        .on_scroll_wheel((e, cx) => {
          this.scroll -= e.delta.y;
          cx.notify();
        })
        .child(v_flex().absolute().left(0).right(0).top(-this.scroll).children(rows)),
      ...pinned,
    ];
  }

  render(cx) {
    const t = theme();
    const list = items(props(), this.git);
    return v_flex()
      .id("menu")
      .track_focus(this.handle)
      .w_full()
      .min_h(0)
      .p(PAD / 2)
      .bg(t.popover)
      .text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.key === "down") this.at = (this.at + 1) % list.length;
        else if (e.key === "up") this.at = (this.at + list.length - 1) % list.length;
        else if (e.key === "enter") this.choose(this.at, cx);
        else if (e.key === "escape") closeOverlay(props().overlay);
        else return;
        this.follow = true;
        cx.notify();
      })
      .children(this.body(list, t));
  }
}
