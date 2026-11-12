// crosstalk inbox: the person's messages, tasks and agents from the crosstalk
// service, and a read-only Status tab (health, stats, recent audit events).
// It is an ordinary crosstalk client: every call goes through the `crosstalk`
// command (`rpc` and `watch --once`), never a socket, file or
// herdr-core document. Message bodies are untrusted text: cleaned, shown
// plain, never typed into a terminal.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { exec, theme, setShared, notify, run, nextCommand } from "herdr";

// The service API methods this mod calls (crosstalk spec v0 section 14).
const METHODS = ["whoami", "fetch", "ack", "nack", "publish", "watch", "task.list", "task.claim", "task.done", "presence.list"];
// The observer methods (people only) only the Status tab calls: a crosstalk
// without them still runs the inbox.
const OBSERVER = ["health", "stats", "events"];
const TABS = ["inbox", "tasks", "agents", "status"];
const STATUS_MS = 5000; // the Status tab re-reads this often while shown
const EVENTS = 20, OPS_SHOWN = 15;
// `crosstalk` exit codes: a response arrived (even an error), bad usage,
// the service can't be reached or started, watch timed out.
const EXIT = { ok: 0, usage: 64, unavailable: 69, timeout: 124 };
const WATCH = "60s"; // the CLI's own timeout: a watch waiting at close() ends by then
const MERGE_MS = 30000; // at most one request toast per window
const PREVIEW = 200, FULL = 64 * 1024, SNOOZE_S = 600;
const TERMINAL = "x-herdr.terminal";
const TOASTED_MAX = 1000;

const firstLine = (s) => String(s ?? "").trim().split("\n")[0] ?? "";

// Turns a `crosstalk rpc` run into { result } or { error: { kind, message, hint } }.
function parseRpc(r) {
  if (r.code === EXIT.ok) {
    let msg;
    try { msg = JSON.parse(r.stdout.trim()); } catch { return { error: { kind: "bug", message: "crosstalk printed something that is not JSON", hint: "" } }; }
    if (msg.error) return { error: { kind: msg.error.data?.kind ?? "unknown", message: msg.error.message ?? "", hint: msg.error.data?.hint ?? "" } };
    return { result: msg.result };
  }
  if (r.code === EXIT.usage) return { error: { kind: "bug", message: "crosstalk refused the mod's call as bad usage", hint: firstLine(r.stderr) } };
  return { error: { kind: "unavailable", message: "crosstalk is not available", hint: firstLine(r.stderr) } };
}

// Turns a `crosstalk watch --once` run into { event } | { timeout } | { error }.
function parseWatch(r) {
  if (r.code === EXIT.timeout) return { timeout: true };
  if (r.code === EXIT.ok) {
    let printed;
    try { printed = JSON.parse(r.stdout.trim()); } catch { return { error: { kind: "bug", message: "crosstalk watch printed something that is not JSON", hint: "" } }; }
    // A service error printed like rpc's (exit 0, an error object) is an error, not an event.
    if (printed && typeof printed === "object" && printed.error) return parseRpc(r);
    return { event: printed };
  }
  return parseRpc(r);
}

async function rpc(method, params = {}) {
  try { return parseRpc(await exec("crosstalk", ["rpc", method, JSON.stringify(params)], { timeout: 15000 })); }
  catch (e) { return { error: { kind: "unavailable", message: "crosstalk could not be started", hint: String(e) } }; }
}

async function watchOnce() {
  try { return parseWatch(await exec("crosstalk", ["watch", "--once", "--json", "--timeout", WATCH], { timeout: 75000 })); }
  catch (e) { return { error: { kind: "unavailable", message: "crosstalk could not be started", hint: String(e) } }; }
}

// Methods the mod needs that rpc.discover does not list.
function missingMethods(discovered) {
  const have = new Set((discovered?.methods ?? []).map((m) => m.name));
  return METHODS.filter((m) => !have.has(m));
}

// Untrusted text for display: no escape sequences, control characters (other
// than newline and tab), bidirectional overrides, line or paragraph
// separators, zero-width spaces, fillers, blanks, or stray tag characters
// and variation selectors; joiners and emoji presentation stay, so emoji
// sequences and scripts that need them read as written; capped with a marker.
function clean(text, max) {
  let s = String(text ?? "")
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?/g, "")
    .replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, "")
    // Tag characters survive only inside a subdivision flag (🏴 + tags + cancel tag).
    .replace(/(\u{1f3f4}[\u{e0061}-\u{e007a}]{2,7}\u{e007f})|[\u{e0000}-\u{e007f}]/gu, (_m, flag) => flag ?? "")
    // ZWNJ, ZWJ and VS15/VS16 (U+200C, U+200D, U+FE0E, U+FE0F) stay: scripts and emoji need them.
    .replace(/[\u00ad\u034f\u061c\u115f\u1160\u17b4\u17b5\u180e\u200b\u200e\u200f\u2028-\u202e\u2060-\u206f\u2800\u3164\ufe00-\ufe0d\ufeff\uffa0]/gu, "");
  if (s.length > max) s = s.slice(0, max) + " … (truncated)";
  return s;
}
const preview = (text) => clean(String(text ?? "").replace(/\s+/g, " ").trim(), PREVIEW);
const bodyText = (envelope) => (typeof envelope?.body?.text === "string" ? envelope.body.text : "");

// A reply: a new message to the original's reply_to (else its sender), in
// its thread, carrying its correlation_id so an `ask` sees the answer.
function replyEnvelope(original, text) {
  const e = {
    spec: original.spec ?? "0.1", kind: "core.message", plane: "data",
    to: [original.reply_to || original.from], at: new Date().toISOString(),
    in_reply_to: original.id, thread: original.thread || original.id, body: { text },
  };
  if (original.correlation_id) e.correlation_id = original.correlation_id;
  return e;
}

// Where a reply goes when that is not the sender: reply_to is the sender's
// choice, so the person sees it before answering.
function replyNote(e) {
  return e.reply_to && e.reply_to !== e.from ? `Reply goes to ${clean(e.reply_to, 200)}` : null;
}

// The service stamps from_verified with from; anything but true is unverified.
const senderLabel = (e) => (e.from_verified === true ? "verified" : "unverified");

// What the person can expect after a failure.
const retryNote = (problem) => (problem?.kind === "unsupported" ? " · reopen the inbox after updating crosstalk" : " · retrying");

const kindLabel = (kind) => ({ "core.request": "request", "core.mail": "mail", "core.message": "message" })[kind] ?? kind;

// --- the Status tab: every value comes from the service and is cleaned ---

const short = (v) => clean(v, 200);
const ms = (n) => (typeof n === "number" && Number.isFinite(n) ? (n < 10 ? n.toFixed(1) : String(Math.round(n))) : "?");
const clock = (at) => { const s = short(at); return /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d/.test(s) ? s.slice(11, 19) : s; };

// health: its status, and each check with why it fails.
function healthView(h) {
  const checks = Array.isArray(h?.checks) ? h.checks : [];
  return {
    ok: h?.status === "ok", status: short(h?.status ?? "unknown"),
    checks: checks.map((c) => ({ name: short(c?.name), ok: c?.ok === true, detail: short(c?.detail ?? "") })),
    about: `pid ${short(h?.pid)} · version ${short(h?.version)} · up since ${short(h?.started_at)}`,
  };
}

// One line per op or histogram, the busiest first, at most OPS_SHOWN.
function opLines(list) {
  const rows = (Array.isArray(list) ? list : []).slice().sort((a, b) => (b?.count ?? 0) - (a?.count ?? 0));
  const unit = (o) => (o?.unit && o.unit !== "ms" ? ` ${short(o.unit)}` : " ms");
  const lines = rows.slice(0, OPS_SHOWN).map((o) => ({
    name: [o?.op, o?.backend, o?.outcome].filter(Boolean).map(short).join(" · "),
    value: `${short(o?.count ?? 0)} · p95 ${ms(o?.p95_ms)} · p99 ${ms(o?.p99_ms)}${unit(o)}`,
  }));
  if (rows.length > OPS_SHOWN) lines.push({ name: `… ${rows.length - OPS_SHOWN} more`, value: "" });
  return lines;
}

const counts = (m) => {
  const entries = Object.entries(m && typeof m === "object" ? m : {}).sort(([a], [b]) => (a < b ? -1 : 1));
  return entries.length ? entries.map(([k, v]) => `${short(k)} ${short(v)}`).join(" · ") : "none";
};

// stats, summarised: what the person reads at a glance, then ops and histograms.
function statsView(s) {
  const st = s?.streams ?? {};
  return {
    summary: [
      { name: "up since", value: short(s?.started_at ?? "?") },
      { name: "open requests", value: short(s?.open_requests ?? 0) },
      { name: "tasks", value: counts(s?.tasks) },
      { name: "inbox depth", value: counts(s?.inbox_depth) },
      { name: "streams", value: `watch ${short(st.watch ?? 0)} · audit ${short(st.audit ?? 0)} · presence ${short(st.presence ?? 0)}` },
    ],
    ops: opLines(s?.ops), histograms: opLines(s?.histograms), at: short(s?.at ?? ""),
  };
}

// An audit event as one line, from its metadata fields only: an event never
// carries a body, and a field this list does not name is never shown.
function eventLine(ev) {
  if (ev?.op === "missed") return { at: clock(ev?.at), text: `missed ${short(ev?.count ?? "?")} events`, warn: true };
  const to = Array.isArray(ev?.to) ? ev.to.map(short).join(", ") : "";
  const parts = [
    `${short(ev?.op)} ${short(ev?.outcome)}`,
    ev?.from ? `${short(ev.from)}${to ? ` → ${to}` : ""}` : to ? `→ ${to}` : "",
    ev?.kind ? kindLabel(short(ev.kind)) : "",
    ev?.grantee ? `grantee ${short(ev.grantee)}` : "",
    Array.isArray(ev?.actions) && ev.actions.length ? ev.actions.map(short).join("+") : "",
    ev?.policy ? `policy ${short(ev.policy)}` : "",
    ev?.granted_by ? `by ${short(ev.granted_by)}` : "",
    ev?.count ? `×${short(ev.count)}` : "",
    `${ms(ev?.duration_ms)} ms`,
    ev?.verified_sender === true ? "" : "unverified",
  ];
  return { at: clock(ev?.at), text: parts.filter(Boolean).join(" · "), warn: ev?.outcome !== "ok" && ev?.outcome !== "noop" };
}

// What the Status tab says when it cannot read.
function statusProblemText(p) {
  if (p.kind === "unauthorised") return `crosstalk did not take this panel for the person (unauthorised): stats, health and events are for the person only${p.hint ? ` · ${p.hint}` : ""}`;
  if (p.kind === "unsupported") return `${p.message}${p.hint ? ` · ${p.hint}` : ""} · reopen the inbox after updating crosstalk`;
  return `${p.message}${p.hint ? ` · ${p.hint}` : ""} · retrying`;
}

export default class Crosstalk extends View {
  init(_props, cx) {
    this.tab = "inbox"; this.me = null; this.problem = null; this.inbox = []; this.tasks = []; this.agents = [];
    this.open = null; this.watching = false; this.closed = false; this.retryAt = 0; this.delay = 1000;
    this.toasted = new Set(); this.pending = new Set(); this.lastToast = -Infinity; this.ready = false;
    this.starting = false; this.reported = new Set(); this.methods = null;
    this.health = null; this.stats = null; this.events = []; this.statusProblem = null; this.statusAt = Infinity; this.statusLoading = false;
    this.input = InputState.new({ placeholder: "Reply…" });
    this.input.on("submit", (_e, cx) => this.reply(cx));
    cx.spawn(async (cx) => { await this.start(cx); });
    cx.timer.every(1000, (cx) => this.tick(cx));
    // Key commands ("<key>": "crosstalk.<command>" in keys.json) while the panel is shown.
    cx.spawn(async (cx) => { try { while (!this.closed) this.command(await nextCommand(), cx); } catch { /* no key commands here */ } });
  }

  command(c, cx) {
    if (this.closed) return;
    if (TABS.includes(c)) this.show(c, cx);
    else if (c === "refresh") { if (this.tab === "status") this.statusAt = 0; this.tick(cx); }
  }

  show(tab, cx) {
    this.tab = tab;
    this.statusAt = tab === "status" ? 0 : Infinity;
    if (tab === "status") this.tick(cx);
    cx.notify();
  }

  async close() { this.closed = true; }

  async start(cx) {
    this.starting = true;
    try { await this.connect(cx); } finally { this.starting = false; }
  }

  async connect(cx) {
    const d = await rpc("rpc.discover");
    if (d.error) return this.fail(d.error, cx);
    this.methods = new Set((d.result?.methods ?? []).map((m) => m.name));
    const missing = missingMethods(d.result);
    if (missing.length) return this.fail({ kind: "unsupported", message: "this crosstalk is too old for the inbox", hint: `missing: ${missing.join(", ")}` }, cx);
    const who = await rpc("whoami");
    if (who.error) return this.fail(who.error, cx);
    if (this.closed) return;
    this.me = who.result; this.ready = true; this.problem = null; this.delay = 1000; this.retryAt = 0;
    await this.refresh(cx);
    this.arm(cx);
  }

  // Every failure backs off (1 s doubling to 60 s), except a crosstalk too
  // old for the mod, which waits for a reload. A bug is reported once.
  fail(error, cx) {
    if (this.closed) return;
    this.problem = { kind: error.kind, message: clean(error.message, 300), hint: clean(error.hint, 300) };
    const key = `${error.kind}:${error.message}`;
    if (error.kind === "bug" && !this.reported.has(key)) {
      this.reported.add(key);
      notify({ title: "crosstalk inbox: bug", body: `${this.problem.message} ${this.problem.hint}`.trim(), level: "error" });
    }
    this.retryAt = error.kind === "unsupported" ? Infinity : Date.now() + this.delay;
    this.delay = Math.min(this.delay * 2, 60000);
    cx.notify();
  }

  tick(cx) {
    if (this.closed) return;
    this.flushToasts();
    if (!this.statusLoading && Date.now() >= this.statusAt) cx.spawn(async (cx) => { await this.loadStatus(cx); });
    if (Date.now() < this.retryAt) return;
    if (!this.ready) { if (!this.starting) cx.spawn(async (cx) => { await this.start(cx); }); return; }
    this.arm(cx);
  }

  // One watch at a time; each event re-reads, each timeout re-arms.
  arm(cx) {
    if (this.closed || this.watching || !this.ready || Date.now() < this.retryAt) return;
    this.watching = true;
    cx.spawn(async (cx) => {
      const w = await watchOnce();
      this.watching = false;
      if (this.closed) return;
      if (w.error) { this.fail(w.error, cx); return; }
      this.delay = 1000; this.problem = null;
      if (w.event) await this.refresh(cx, w.event.type);
      this.arm(cx);
    });
  }

  async refresh(cx, what) {
    if (this.closed) return;
    const all = !what || what === "missed";
    if (all || what === "message" || what === "ctl.receipt") {
      const f = await rpc("fetch", { limit: 200 });
      if (f.error) return this.fail(f.error, cx);
      this.setInbox(f.result ?? []);
    }
    if (this.closed) return;
    if (all || what === "ctl.presence") {
      const p = await rpc("presence.list");
      if (this.closed) return;
      if (!p.error) this.agents = p.result ?? [];
    }
    if (this.closed) return;
    if (all) {
      const t = await rpc("task.list");
      if (this.closed) return;
      if (!t.error) this.tasks = Array.isArray(t.result) ? t.result : t.result?.tasks ?? [];
    }
    if (!this.closed) cx.notify();
  }

  // The Status tab's read: health, stats, then the recent events, stopping
  // at the first failure; the next read is STATUS_MS later, whatever happened.
  async loadStatus(cx) {
    if (this.closed || this.statusLoading) return;
    this.statusLoading = true; this.statusAt = Date.now() + STATUS_MS;
    try {
      const missing = this.methods ? OBSERVER.filter((m) => !this.methods.has(m)) : [];
      // No call is made for a too-old crosstalk: it waits for a reopen.
      if (missing.length) return this.statusFail({ kind: "unsupported", message: "this crosstalk has no observer methods", hint: `missing: ${missing.join(", ")}` });
      const h = await rpc("health");
      if (this.closed) return;
      if (h.error) return this.statusFail(h.error);
      const s = await rpc("stats");
      if (this.closed) return;
      if (s.error) return this.statusFail(s.error);
      const e = await rpc("events", { limit: EVENTS });
      if (this.closed) return;
      if (e.error) return this.statusFail(e.error);
      this.health = h.result; this.stats = s.result;
      this.events = (Array.isArray(e.result) ? e.result : []).slice(-EVENTS).reverse();
      this.statusProblem = null;
    } finally {
      this.statusLoading = false;
      if (!this.closed) cx.notify();
    }
  }

  // The last good read stays, marked by its time.
  statusFail(error) {
    if (this.closed) return;
    this.statusProblem = { kind: error.kind, message: clean(error.message, 300), hint: clean(error.hint, 300) };
    const key = `${error.kind}:${error.message}`;
    if (error.kind === "bug" && !this.reported.has(key)) {
      this.reported.add(key);
      notify({ title: "crosstalk inbox: bug", body: `${this.statusProblem.message} ${this.statusProblem.hint}`.trim(), level: "error" });
    }
  }

  setInbox(deliveries) {
    if (this.closed) return;
    this.inbox = deliveries.filter((d) => d.state !== "acked");
    if (this.open && !this.inbox.some((d) => d.envelope.id === this.open)) this.open = null;
    setShared("crosstalk.unread", this.inbox.length);
    const ids = new Set(this.inbox.map((d) => d.envelope.id));
    for (const id of this.pending) if (!ids.has(id)) this.pending.delete(id);
    // A request toasts once, even if it leaves the inbox (snoozed) and comes
    // back; the oldest are forgotten past TOASTED_MAX.
    for (const id of this.toasted) { if (this.toasted.size <= TOASTED_MAX) break; this.toasted.delete(id); }
    for (const d of this.inbox) if (d.envelope.kind === "core.request" && !this.toasted.has(d.envelope.id)) this.pending.add(d.envelope.id);
    this.flushToasts();
  }

  // New requests toast together, at most once per merge window; ones that
  // arrive inside a window wait for its end (tick), never dropped.
  flushToasts() {
    if (!this.pending.size || Date.now() - this.lastToast < MERGE_MS) return;
    const requests = this.inbox.filter((d) => this.pending.has(d.envelope.id));
    for (const d of requests) this.toasted.add(d.envelope.id);
    this.pending.clear();
    if (!requests.length) return;
    this.lastToast = Date.now();
    notify({ title: requests.length === 1 ? `Request from ${preview(requests[0].envelope.from)}${requests[0].envelope.from_verified === true ? "" : " (unverified)"}` : `${requests.length} requests`, body: requests.length === 1 ? preview(bodyText(requests[0].envelope)) : "", level: "info", action: { name: "mod.open", args: { name: "crosstalk" } } });
  }

  act(method, params, cx, after) {
    cx.spawn(async (cx) => {
      const r = await rpc(method, params);
      if (this.closed) return;
      if (r.error) this.note = clean(`${method}: ${r.error.kind}${r.error.hint ? ` (${r.error.hint})` : ""}`, 300);
      else { this.note = null; after?.(); }
      await this.refresh(cx);
    });
  }

  reply(cx) {
    const text = this.input.value().trim(), d = this.inbox.find((x) => x.envelope.id === this.open);
    if (!text || !d) return;
    this.act("publish", { envelope: replyEnvelope(d.envelope, text) }, cx, () => this.input.set_value(""));
  }

  terminalOf(address) {
    return this.agents.find((a) => a.address === address)?.ext?.[TERMINAL] ?? null;
  }

  // --- drawing ---

  row(d, t) {
    const e = d.envelope, open = this.open === e.id;
    return v_flex().id(`msg-${e.id}`).flex_none().px(10).py(6).gap(4).rounded(7).cursor_pointer().when(open, (el) => el.bg(t.selected)).hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => { this.open = open ? null : e.id; cx.notify(); })
      .child(h_flex().gap(8)
        .child(div().px(5).rounded(4).border_1().border_color(e.kind === "core.request" ? t.blocked : t.border).text_size(10).text_color(t.muted).child(kindLabel(clean(e.kind, 40))))
        // The sender is drawn outside the body, in the mod's own colours, so a body cannot imitate it.
        .child(div().text_size(11).text_color(t.accent).truncate().child(clean(e.from, 200)))
        .child(div().text_size(10).text_color(e.from_verified === true ? t.dim : t.blocked).child(senderLabel(e)))
        .child(div().flex_1())
        .child(div().text_size(10).text_color(t.dim).child(clean(e.at, 40))))
      .child(div().text_size(12).text_color(t.foreground).when(!open, (el) => el.truncate()).child(open ? clean(bodyText(e), FULL) : preview(bodyText(e))));
  }

  openActions(t) {
    const d = this.inbox.find((x) => x.envelope.id === this.open);
    if (!d) return null;
    const e = d.envelope, term = this.terminalOf(e.from);
    const button = (id, label, on) => div().id(id).h(26).px(10).flex().items_center().rounded(6).border_1().border_color(t.border).cursor_pointer().hover((s) => s.bg(t.hover)).text_size(11).on_click(on).child(label);
    const goes = replyNote(e);
    return v_flex().flex_none().p(10).gap(8).border_t(1).border_color(t.border)
      .when(Boolean(goes), (el) => el.child(div().text_size(11).text_color(t.blocked).child(goes)))
      .child(h_flex().gap(6)
        .child(button("ack", "Done", (_e, cx) => this.act("ack", { id: e.id }, cx)))
        .child(button("snooze", "Snooze 10 min", (_e, cx) => this.act("nack", { id: e.id, delay: SNOOZE_S }, cx)))
        .when(Boolean(term), (el) => el.child(button("pane", "Open sender's pane", () => run("pane.focus", { terminal: term })))))
      .child(div().h(30).px(10).flex().items_center().rounded(8).border_1().border_color(t.border).cursor_text().on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.input).h_full().text_size(12)));
  }

  statusView(t) {
    const head = (text) => div().px(10).pt(10).pb(2).text_size(11).font_semibold().text_color(t.muted).child(text);
    const dim = (text) => div().px(10).py(2).text_size(12).text_color(t.dim).child(text);
    const line = (id, name, value, color) => h_flex().id(id).px(10).py(2).gap(8)
      .child(div().w(160).flex_none().truncate().text_size(11).text_color(t.muted).child(name))
      .child(div().flex_1().truncate().text_size(12).text_color(color ?? t.foreground).child(value));
    const lines = (prefix, rows) => (rows.length ? rows.map((r, i) => line(`${prefix}-${i}`, r.name, r.value)) : [dim("none yet")]);
    const items = [];
    const p = this.statusProblem;
    if (p) items.push(div().p(10).text_size(12).text_color(p.kind === "unauthorised" ? t.blocked : t.dim).child(statusProblemText(p)));
    if (!this.health && !this.stats) {
      if (!p) items.push(dim("Reading crosstalk…"));
      return v_flex().children(items);
    }
    if (this.health) {
      const h = healthView(this.health);
      items.push(h_flex().px(10).pt(10).gap(8).items_center()
        .child(div().font_semibold().text_color(h.ok ? t.foreground : t.blocked).child(h.ok ? "Healthy" : `Health: ${h.status}`))
        .child(div().text_size(10).text_color(t.dim).truncate().child(h.about)));
      items.push(...h.checks.map((c, i) => line(`check-${i}`, c.name, c.ok ? "ok" : `failing${c.detail ? `: ${c.detail}` : ""}`, c.ok ? t.dim : t.blocked)));
    }
    if (this.stats) {
      const s = statsView(this.stats);
      items.push(head(`Stats${s.at ? ` · as of ${s.at}` : ""}`), ...lines("sum", s.summary));
      items.push(head("Ops (count · p95 · p99)"), ...lines("op", s.ops));
      items.push(head("Histograms"), ...lines("hist", s.histograms));
    }
    items.push(head("Recent events (newest first, metadata only)"));
    items.push(...(this.events.length ? this.events.map((ev, i) => {
      const l = eventLine(ev);
      return h_flex().id(`event-${i}`).px(10).py(2).gap(8)
        .child(div().w(60).flex_none().text_size(10).text_color(t.dim).child(l.at))
        .child(div().flex_1().truncate().text_size(11).text_color(l.warn ? t.blocked : t.foreground).child(l.text));
    }) : [dim("none yet")]));
    return v_flex().children(items);
  }

  render(cx) {
    const t = theme();
    const tabBtn = (id, label) => div().id(`tab-${id}`).px(10).py(4).rounded(6).cursor_pointer().text_size(12).when(this.tab === id, (e) => e.bg(t.selected)).hover((s) => s.bg(t.hover)).on_click((_e, cx) => this.show(id, cx)).child(label);
    const empty = (text) => div().p(10).text_color(t.dim).text_size(12).child(text);
    let body;
    if (this.tab === "status") body = this.statusView(t);
    else if (this.problem && !this.ready) body = empty(`${this.problem.message}${this.problem.hint ? ` · ${this.problem.hint}` : ""}${retryNote(this.problem)}`);
    else if (this.tab === "inbox") body = this.inbox.length ? v_flex().children(this.inbox.map((d) => this.row(d, t))) : empty(this.ready ? "No messages" : "Connecting…");
    else if (this.tab === "tasks") body = this.tasks.length ? v_flex().children(this.tasks.map((k) => h_flex().id(`task-${k.id}`).px(10).py(6).gap(8)
      .child(div().w(80).text_size(10).text_color(t.muted).child(clean(k.status, 20)))
      .child(div().flex_1().truncate().text_size(12).child(preview(k.title)))
      .when(k.status === "open", (el) => el.child(div().id(`claim-${k.id}`).px(8).rounded(5).border_1().border_color(t.border).cursor_pointer().text_size(11).on_click((_e, cx) => this.act("task.claim", { id: k.id, expect_status: "open", expect_version: k.version }, cx)).child("Claim")))
      .when(k.assignee === this.me?.address && ["claimed", "in_progress", "in_review"].includes(k.status), (el) => el.child(div().id(`done-${k.id}`).px(8).rounded(5).border_1().border_color(t.border).cursor_pointer().text_size(11).on_click((_e, cx) => this.act("task.done", { id: k.id }, cx)).child("Done"))))) : empty("No tasks");
    else body = this.agents.length ? v_flex().children(this.agents.map((a) => h_flex().px(10).py(6).gap(8)
      .child(div().w(70).text_size(10).text_color(t[a.state] ?? t.muted).child(clean(a.state, 20)))
      .child(div().flex_1().truncate().text_size(12).child(clean(a.address, 200)))
      .child(div().text_size(10).text_color(t.dim).child(a.verified ? "verified" : "unverified"))
      .when(Boolean(a.note), (el) => el.child(div().max_w(200).truncate().text_size(10).text_color(t.dim).child(preview(a.note)))))) : empty("No agents");
    return v_flex().id("crosstalk").size_full().min_h(0).bg(t.background).text_color(t.foreground).text_size(13)
      .child(h_flex().h(40).flex_none().px(12).gap(6).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child("crosstalk"))
        .child(div().text_size(10).text_color(t.dim).truncate().child(this.me ? `${clean(this.me.address, 200)}${this.me.verified ? "" : " (unverified)"}` : ""))
        .child(div().flex_1())
        .child(tabBtn("inbox", this.inbox.length ? `Inbox ${this.inbox.length}` : "Inbox")).child(tabBtn("tasks", "Tasks")).child(tabBtn("agents", "Agents")).child(tabBtn("status", "Status")))
      .child(div().id("crosstalk-body").flex_1().min_h(0).overflow_y_scroll().p(6).child(body))
      .when(this.tab === "inbox" && Boolean(this.open), (el) => el.child(this.openActions(t)))
      .when(Boolean(this.note), (el) => el.child(div().px(12).pb(8).text_size(11).text_color(t.ansi1).child(this.note)))
      .when(Boolean(this.problem && this.ready), (el) => el.child(div().px(12).pb(8).text_size(11).text_color(t.dim).child(`${this.problem.message}${retryNote(this.problem)}`)));
  }
}
