// How notifications look. Anything can post one with notify({ title, body,
// level, icon, action }); replace this mod to restyle them all. Click to
// run its action (e.g. go to the agent) and dismiss it; the × that shows on
// hover (or the Dismiss button) only dismisses it. Herdr draws the box
// (surface, border, corners: toast_style, toast_corners, toast_design) and
// sizes and places it; this mod draws the inside, in the design Herdr puts
// in the props (props().toast_design), with the level mark (props().toast_mark).
// With props().fitHeight the toast grows to show its whole text, wrapped, from
// props().toast_height up; without it each line is one line, cut short.
// Every colour is one of the theme's toast colours (settings.rs, toast_colors).

import { View, div, image, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { props, closeOverlay, run, theme } from "herdr";

// Each level's color, held to 3:1 on the toast's surface (settings.rs,
// toast_colors), or 4.5:1 where the design draws it as text (terminal, outline).
const LEVEL = { error: "toast_error", warning: "toast_warning", success: "toast_success", info: "toast_info" };
// The terminal design's bracketed level.
const TAG = { error: "ERR", warning: "WARN", success: "DONE", info: "INFO" };
// The icon card's level icon, mods/toast/<name>.svg.
const ICON = { error: "error", warning: "warning", success: "check", info: "info" };

// Blank text is no body; window.rs's toast_height decides the same way.
const hasBody = (body) => (typeof body === "string" ? body.trim() !== "" : Boolean(body));

export default class Toast extends View {
  render(cx) {
    const t = theme();
    const p = props() ?? {};
    const levelName = LEVEL[p.level] ? p.level : "info";
    const level = t[LEVEL[levelName]];
    const design = p.toast_design ?? "card";
    // The level mark, which Herdr puts in the props as the design takes the
    // person's toast_mark (a pill's stripe is a dot; the outline's border is
    // its mark): "stripe" (the default) is a stripe in the level's color down
    // the left, its width taken from the padding, and the dot; "dot": the dot
    // alone; "none": neither (an icon still shows).
    const mark = p.toast_mark ?? "stripe";
    const act = () => {
      if (p.action?.name) run(p.action.name, p.action.args ?? {});
      closeOverlay(p.overlay);
    };
    // A machine that needs a login says the command to run, with a button that copies it.
    const login = /^([a-z0-9][a-z0-9_.-]{0,31}) needs login; run hc machine login \1$/.exec(p.body ?? "");
    const command = login ? `hc machine login ${login[1]}` : null;
    const titleText = login ? `${login[1]} needs a login: run` : p.title ?? "";
    // The whole text when the toast grows to fit it, else its first line, cut short.
    const text = (value, style) => p.fitHeight
      ? style(div()).child(value)
      : style(div().truncate()).child(typeof value === "string" ? value.split("\n", 1)[0] : value);
    // A root that fills a box of set height, or one at least the design's height that grows with its text.
    const sized = (el) => p.fitHeight ? el.min_h(p.toast_height ?? 72).flex_none().py(8) : el.size_full();
    const title = (color) => text(titleText, (el) => el.font_semibold().text_size(13).text_color(color));
    const copy = () =>
      div()
        .id("toast-login-copy")
        .flex_none()
        .px(6)
        .py(3)
        .rounded(4)
        .border_1()
        .border_color(t.toast_border)
        .text_size(12)
        .text_color(t.toast_foreground)
        .cursor_pointer()
        .on_click((_e, cx) => {
          cx.stop_propagation();
          cx.write_to_clipboard(command);
          this.copied = true;
          cx.notify();
        })
        .child(this.copied ? "Copied" : "Copy");
    const body = () =>
      login
        ? h_flex().gap(8).items_center().child(div().flex_1().min_w(0).truncate().tooltip(command).text_size(12).text_color(t.toast_muted).child(command)).child(copy())
        : text(p.body ?? "", (el) => el.text_size(12).text_color(t.toast_muted));
    const lead = (el) =>
      el
        .when(Boolean(p.icon), (el) => el.child(image(p.icon).w(18).h(18).flex_none()))
        .when(!p.icon && mark !== "none", (el) => el.child(div().id("toast-dot").size(8).rounded(4).flex_none().bg(level)));
    // The other designs' root; the card's and outline's is below.
    const root = sized(h_flex().id("toast"))
      .items_center()
      .cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click(act);
    // Show runs the action, filled in the level's colour; Dismiss only closes.
    const button = (id, label, onClick, filled) =>
      div()
        .id(id)
        .px(10)
        .py(3)
        .rounded(5)
        .text_size(12)
        .font_semibold()
        .cursor_pointer()
        .text_color(filled ? t.toast : t.toast_foreground)
        .when(filled, (el) => el.bg(level))
        .when(!filled, (el) => el.border_1().border_color(t.toast_border))
        .on_click((_e, cx) => {
          cx.stop_propagation();
          onClick();
        })
        .child(label);
    const show = () => button("toast-show", p.action?.label ?? "Show", act, true);
    const dismiss = () => button("toast-dismiss", "Dismiss", () => closeOverlay(p.overlay), false);
    // A × in the corner while the pointer is over the toast: it closes without
    // running the action, which a click on the toast itself runs. Designs that
    // draw a Dismiss button don't get one.
    const closable = (el) =>
      el
        .relative()
        .on_hover((hovered, cx) => {
          this.hovered = hovered;
          cx.notify();
        })
        .when(Boolean(this.hovered), (el) =>
          el.child(
            div()
              .id("toast-close")
              .absolute()
              .top(design === "pill" ? 8 : 6)
              .right(6)
              .size(16)
              .rounded(4)
              .flex()
              .items_center()
              .justify_center()
              .text_size(12)
              .text_color(t.toast_muted)
              .cursor_pointer()
              .hover((s) => s.bg(t.hover).text_color(t.toast_foreground))
              .on_click((_e, cx) => {
                cx.stop_propagation();
                closeOverlay(p.overlay);
              })
              .child("×"),
          ),
        );

    if (design === "pill") {
      // One line: the mark or icon and the title; the body is its tooltip.
      return closable(
        lead(root.px(14).gap(8).bg(t.toast))
          .when(hasBody(p.body), (el) => el.tooltip(String(p.body)))
          .child(title(t.toast_foreground).flex_1()),
      );
    }
    if (design === "terminal") {
      // The terminal's font (Herdr puts it in the props) and its colour for the level, in brackets.
      return closable(
        root
          .px(12)
          .gap(8)
          .bg(t.toast)
          .when(Boolean(p.toast_font), (el) => el.font_family(p.toast_font))
          .child(div().id("toast-tag").flex_none().font_semibold().text_size(13).text_color(level).child(`[${TAG[levelName]}]`))
          .child(
            v_flex()
              .id("toast-text")
              .flex_1()
              .overflow_hidden()
              .child(text(titleText, (el) => el.text_size(13).text_color(t.toast_foreground)))
              .when(hasBody(p.body), (el) => el.child(body())),
          ),
      );
    }
    if (design === "icon-card") {
      // A big level icon (the toast's own icon if it has one), and Show and Dismiss for an action.
      const icon = p.icon
        ? image(p.icon).w(32).h(32).flex_none()
        : div()
            .id("toast-icon")
            .size(36)
            .rounded(18)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .bg(level)
            .child(svg(`mods/toast/${ICON[levelName]}.svg`).size(20).text_color(t.toast));
      const card = root
        .px(16)
        .gap(14)
        .bg(t.toast)
        .child(icon)
        .child(
          v_flex()
            .id("toast-text")
            .flex_1()
            .gap(2)
            .overflow_hidden()
            .child(title(t.toast_foreground))
            .when(hasBody(p.body), (el) => el.child(body()))
            .when(Boolean(p.action?.name), (el) => el.child(h_flex().id("toast-actions").pt(8).gap(8).child(show()).child(dismiss()))),
        );
      return p.action?.name ? card : closable(card);
    }
    if (design === "banner") {
      // Across the window: the title, then the place or body, how many more wait behind it, and its buttons.
      return lead(root.pl(mark === "stripe" ? 13 : 16).pr(10).gap(10).bg(t.toast))
        .when(mark === "stripe", (el) => el.border_l(3).border_color(level))
        .child(title(t.toast_foreground).min_w(0))
        .child(hasBody(p.body) ? body().flex_1().min_w(0) : div().flex_1())
        .when(Number(p.toast_more) > 0, (el) => el.child(div().id("toast-more").flex_none().text_size(12).text_color(t.toast_muted).child(`+${p.toast_more} more`)))
        .when(Boolean(p.action?.name), (el) => el.child(show()))
        .child(dismiss());
    }
    // Card, and outline: the card's layout (built in the order it always was),
    // see-through (Herdr draws the backing and the level border), its title in
    // the level's colour, with no hover fill, which would lighten what shows
    // through the backing settings.rs fits the text to (toast_colors, toast_ghost).
    const outline = design === "outline";
    return closable(lead(
      sized(h_flex().id("toast"))
        .pl(mark === "stripe" ? 15 : 18)
        .pr(18)
        .gap(12)
        .items_center()
        .when(mark === "stripe", (el) => el.border_l(3).border_color(level))
        .when(!outline, (el) => el.bg(t.toast))
        .cursor_pointer()
        .when(!outline, (el) => el.hover((s) => s.bg(t.hover)))
        .on_click(act),
    ).child(
      v_flex()
        .flex_1()
        .overflow_hidden()
        .child(title(outline ? level : t.toast_foreground))
        .when(hasBody(p.body), (el) => el.child(body())),
    ));
  }
}
