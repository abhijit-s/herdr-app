// Agent alerts in the system's notification center. Herdr runs this mod in
// every layout without drawing it (a background mod) and posts no system
// notification itself; replace this mod to change when or how they go.
//
// One notification per agent (tag "agent:<address>"), for "needs you",
// "hit an error" and "finished", as settings.json's system_notifications says
// (background: only while Herdr isn't the window in front; always; off), and
// never for the agent the person is looking at. A "needs you" one stays until
// the agent is no longer blocked (notification_sticky, on unless false); the
// others go once the person looks at the agent or it works again, and every
// one goes with its agent. Clicking one goes to the agent. Herdr gives each
// the person's notification sounds, marks and icon for its state (`agent`).

import { View, div } from "gpui-kit";
import { agents, terminals, focused, panes, window, settings, systemNotify, withdrawSystemNotification, nextSystemNotificationClick, run } from "herdr";

const BLOCKED = ["blocked", "error"];
const BUSY = ["working", "blocked", "error"];
const VERB = { blocked: "needs you", error: "hit an error", done: "finished" };
const TAG = "agent:";

// Herdr's attention rule for a change of word: needing the person always
// alerts; finishing only if they weren't looking (model.rs, Model::alert).
function alertFor(prev, word, looking) {
  if (BLOCKED.includes(word) && !BLOCKED.includes(prev)) return word === "error" ? "error" : "blocked";
  if (BUSY.includes(prev) && !BUSY.includes(word) && !looking) return "done";
  return null;
}

function allowed(mode, active) {
  if (mode === "off") return false;
  if (mode === "always") return true;
  return !active;
}

// What the person sees of each agent now, by address: its word (the agents
// list's status, "error" told apart from "blocked" by the core's own word),
// whether it is on screen in the window in front, and its notification.
function observe({ agents, terminals, focused, panes, active }) {
  const records = new Map(terminals.map((t) => [t.address, t]));
  const shownTab = focused?.tab;
  const onScreen = (address) =>
    focused?.terminal === address || focused?.agent === address || panes.some((p) => p.terminal === address && p.tab === shownTab);
  const seen = new Map();
  for (const a of agents) {
    const rec = records.get(a.address);
    // Gone from the core's listing: gone. A sub-agent reports to its parent;
    // only top agents ring.
    if (!rec || rec.parent) continue;
    const word = rec?.status === "error" ? "error" : a.status;
    const report = rec?.report && typeof rec.report === "object" ? rec.report : {};
    const app = a.app || (a.program?.agent ? a.program.key : null);
    const name = rec?.name || (app ? a.program?.name : null) || "An agent";
    const place = a.space_name && a.tab_name ? `${a.space_name} / ${a.tab_name}` : "";
    const said = typeof report.msg === "string" ? report.msg.trim() : "";
    seen.set(a.address, {
      word,
      looking: Boolean(active) && onScreen(a.address),
      app,
      name,
      body: [said, place].filter(Boolean).join("\n"),
    });
  }
  return seen;
}

// One look at the agents: what to post and take down. `state` carries what
// this mod saw last and which notifications it has up.
function step(state, seen, { mode, active, sticky }) {
  const actions = [];
  const withdraw = (address) => {
    state.posted.delete(address);
    actions.push({ withdraw: address });
  };
  if (!state.started) {
    // Before the first listing there are no terminals. On it, what an earlier
    // run (the core outlived it) may have left up is adopted as the agent's
    // state would have posted it, and any other is taken down.
    if (seen.size === 0) return actions;
    state.started = true;
    for (const [address, a] of seen) {
      if (BLOCKED.includes(a.word)) state.posted.set(address, a.word);
      else if (a.word === "done") state.posted.set(address, "done");
      else actions.push({ withdraw: address });
      state.words.set(address, a.word);
    }
    // Then as on every look: an adopted one the person sees may go now.
  }
  for (const [address, a] of seen) {
    const up = state.posted.get(address);
    if (up) {
      const stale = sticky && up === "blocked" ? !BLOCKED.includes(a.word) : a.looking || a.word === "working";
      if (stale) withdraw(address);
    }
    const alert = alertFor(state.words.get(address), a.word, a.looking);
    if (alert && !a.looking && allowed(mode, active)) {
      state.posted.set(address, alert);
      actions.push({ post: address, alert, title: `${a.name} ${VERB[alert]}`, body: a.body, app: a.app });
    }
    state.words.set(address, a.word);
  }
  for (const address of [...state.posted.keys()]) if (!seen.has(address)) withdraw(address);
  for (const address of [...state.words.keys()]) if (!seen.has(address)) state.words.delete(address);
  return actions;
}

function apply(actions) {
  for (const a of actions) {
    if (a.withdraw) withdrawSystemNotification(TAG + a.withdraw);
    else systemNotify({ tag: TAG + a.post, title: a.title, body: a.body, agent: { app: a.app, state: a.alert } });
  }
}

export default class Notifications extends View {
  init(_props, cx) {
    this.state = { started: false, words: new Map(), posted: new Map() };
    cx.spawn(async () => {
      for (;;) {
        const { tag } = await nextSystemNotificationClick();
        if (!tag.startsWith(TAG)) continue;
        const terminal = tag.slice(TAG.length);
        // The notification center took it down itself.
        this.state.posted.delete(terminal);
        run("agent.focus", { terminal });
      }
    });
  }

  render() {
    const s = settings() ?? {};
    const active = window()?.active ?? false;
    const seen = observe({ agents: agents() ?? [], terminals: terminals() ?? [], focused: focused(), panes: panes() ?? [], active });
    apply(step(this.state, seen, { mode: s.system_notifications ?? "background", active, sticky: s.notification_sticky !== false }));
    return div();
  }
}
