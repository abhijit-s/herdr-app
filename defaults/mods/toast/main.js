// How notifications look. Anything can post one with notify({ title, body,
// level, icon, action }); replace this mod to restyle them all. Click to
// run its action (e.g. go to the agent) and dismiss it. Herdr draws the box
// (surface, border, corners: toast_style, toast_corners); this mod draws the
// level mark (toast_mark) and the text.

import { View, div, image } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { props, closeOverlay, run, theme, settings } from "herdr";

// Each level's color, held to 3:1 on the toast's surface (settings.rs, toast_colors).
const LEVEL = { error: "toast_error", warning: "toast_warning", success: "toast_success", info: "toast_info" };

// Blank text is no body; window.rs's toast_height decides the same way.
const hasBody = (body) => (typeof body === "string" ? body.trim() !== "" : Boolean(body));

export default class Toast extends View {
  render(cx) {
    const t = theme();
    const p = props() ?? {};
    const level = t[LEVEL[p.level] ?? "toast_info"];
    // "stripe" (the default): a stripe in the level's color down the left, its
    // width taken from the padding, and the dot; "dot": the dot alone; "none":
    // neither (an icon still shows).
    const mark = settings()?.toast_mark ?? "stripe";
    return h_flex()
      .id("toast")
      .size_full()
      .pl(mark === "stripe" ? 15 : 18)
      .pr(18)
      .gap(12)
      .items_center()
      .when(mark === "stripe", (el) => el.border_l(3).border_color(level))
      .bg(t.toast)
      .cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click(() => {
        if (p.action?.name) run(p.action.name, p.action.args ?? {});
        closeOverlay(p.overlay);
      })
      .when(Boolean(p.icon), (el) => el.child(image(p.icon).w(18).h(18).flex_none()))
      .when(!p.icon && mark !== "none", (el) => el.child(div().id("toast-dot").size(8).rounded(4).flex_none().bg(level)))
      .child(
        v_flex()
          .flex_1()
          .overflow_hidden()
          .child(div().truncate().font_semibold().text_size(13).text_color(t.toast_foreground).child(p.title ?? ""))
          .when(hasBody(p.body), (el) => el.child(div().truncate().text_size(12).text_color(t.toast_muted).child(p.body ?? ""))),
      );
  }
}
