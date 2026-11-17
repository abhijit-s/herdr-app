import { beforeEach, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The notifications mod without a window: what it posts and takes down as the
// agents change, through the herdr calls it makes.
let now: Record<string, any> = {};
let calls: any[][] = [];
let clicks: ((v: { tag: string }) => void)[] = [];
const values: Record<string, any> = {
  View: class {},
  div: () => ({}),
  agents: () => now.agents,
  terminals: () => now.terminals,
  focused: () => now.focused,
  panes: () => now.panes,
  window: () => now.window,
  settings: () => now.settings,
  systemNotify: (n: any) => calls.push(["post", n]),
  withdrawSystemNotification: (tag: string) => calls.push(["withdraw", tag]),
  nextSystemNotificationClick: () => new Promise((resolve) => clicks.push(resolve)),
  run: (name: string, args: any) => calls.push(["run", name, args]),
};
const source = await readFile(new URL("../defaults/mods/notifications/main.js", import.meta.url), "utf8");
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap((m) => m[1].split(",").map((n) => n.trim()));
const body = source.replace(/^import .*;\n/gm, "").replace("export default class Notifications", "const Notifications = class Notifications") + "\nreturn Notifications;";
const Notifications = new Function(...names, body)(...names.map((n) => values[n]));

// One agent, t1, in the shown tab's pane, or none.
const agent = (status: string, extra: Record<string, any> = {}) => ({
  address: "t1", pane: "x1", tab: "c1", space_name: "api", tab_name: "one", status, app: "claude-code",
  program: { key: "claude", name: "Claude Code", agent: true }, ...extra,
});
const world = (status: string | null, { active = false, onScreen = true, raw, msg, mode, sticky }: Record<string, any> = {}) => {
  now = {
    agents: status ? [agent(status)] : [],
    terminals: status ? [{ address: "t1", status: raw ?? status, report: msg ? { msg } : undefined }] : [],
    focused: { tab: onScreen ? "c1" : "c2", terminal: null, agent: null },
    panes: [{ pane: "x1", tab: "c1", terminal: "t1" }],
    window: { active },
    settings: { system_notifications: mode, notification_sticky: sticky },
  };
};
let mod: any;
const look = (status: string | null, opts: Record<string, any> = {}) => {
  world(status, opts);
  calls = [];
  mod.render();
  return calls.map(([kind, what]) => (kind === "post" ? `post ${what.title}` : kind === "withdraw" ? `withdraw ${what}` : kind));
};
beforeEach(() => {
  clicks = [];
  mod = new Notifications();
  mod.init({}, { spawn: (f: () => Promise<void>) => f() });
});

test("the first listing is adopted, not news", () => {
  expect(look(null)).toEqual([]);
  expect(look("blocked")).toEqual([]);
  expect(look("working")).toEqual(["withdraw agent:t1"], "the adopted needs-you goes once unblocked");
  mod = new Notifications();
  mod.init({}, { spawn: () => {} });
  expect(look("idle")).toEqual(["withdraw agent:t1"], "anything else left from before goes");
});

test("needing the person posts once, and stays until the agent is unblocked", () => {
  look("working");
  expect(look("blocked")).toEqual(["post Claude Code needs you"]);
  const posted = calls[0][1];
  expect(posted).toEqual({ tag: "agent:t1", title: "Claude Code needs you", body: "api / one", agent: { app: "claude-code", state: "blocked" } });
  expect(look("blocked", { active: true })).toEqual([], "seen: it stays");
  expect(look("working")).toEqual(["withdraw agent:t1"]);
  expect(look("blocked", { active: true })).toEqual([], "blocked while looking: nothing");
});

test("not sticky, seeing a needs-you takes it down", () => {
  look("working", { sticky: false });
  expect(look("blocked", { sticky: false })).toEqual(["post Claude Code needs you"]);
  expect(look("blocked", { sticky: false, active: true })).toEqual(["withdraw agent:t1"]);
});

test("an error is its own alert, with the agent's message over the place", () => {
  look("working");
  expect(look("blocked", { raw: "error", msg: "  rate limited  " })).toEqual(["post Claude Code hit an error"]);
  expect(calls[0][1].body).toBe("rate limited\napi / one");
  expect(calls[0][1].agent.state).toBe("error");
  expect(look("blocked", { raw: "error", active: true })).toEqual(["withdraw agent:t1"], "an error goes once seen");
});

test("finishing unseen posts; a needs-you unblocked unseen becomes a finish", () => {
  look("working");
  expect(look("done")).toEqual(["post Claude Code finished"]);
  expect(look("idle", { active: true })).toEqual(["withdraw agent:t1"]);
  look("working");
  look("blocked");
  expect(look("done")).toEqual(["withdraw agent:t1", "post Claude Code finished"]);
  look("working");
  expect(look("idle", { active: true })).toEqual([], "finished while looking: nothing");
});

test("system_notifications decides when, never for the agent on screen", () => {
  look("working");
  expect(look("blocked", { active: true, onScreen: false })).toEqual([], "background: not while Herdr is in front");
  look("working", { mode: "always" });
  expect(look("blocked", { mode: "always", active: true, onScreen: false })).toEqual(["post Claude Code needs you"]);
  look("working", { mode: "off" });
  expect(look("blocked", { mode: "off" })).toEqual([], "off: nothing");
});

test("an agent that goes takes its notification", () => {
  look("working");
  look("blocked");
  expect(look(null)).toEqual(["withdraw agent:t1"]);
});

test("a sub-agent never rings", () => {
  look("working");
  world("blocked");
  now.terminals[0].parent = "t0";
  calls = [];
  mod.render();
  expect(calls).toEqual([]);
});

test("a click goes to the agent", async () => {
  look("working");
  look("blocked");
  calls = [];
  clicks.shift()!({ tag: "agent:t1" });
  await Promise.resolve();
  await Promise.resolve();
  expect(calls).toEqual([["run", "agent.focus", { terminal: "t1" }]]);
  expect(look("working")).toEqual([], "the notification center took it down itself");
});

test("an agent without a name or an app is an agent", () => {
  look("working");
  world("blocked");
  now.agents[0] = { ...now.agents[0], app: null, program: { key: "program", name: "node", agent: false }, space_name: "", tab_name: "" };
  calls = [];
  mod.render();
  expect(calls[0][1]).toEqual({ tag: "agent:t1", title: "An agent needs you", body: "", agent: { app: null, state: "blocked" } });
});

test("adopted from an earlier run: an error or a finish goes once seen, even on the first look", () => {
  expect(look("blocked", { raw: "error" })).toEqual([]);
  expect(look("blocked", { raw: "error", active: true })).toEqual(["withdraw agent:t1"]);
  mod = new Notifications();
  mod.init({}, { spawn: () => {} });
  expect(look("done", { active: true })).toEqual(["withdraw agent:t1"], "seen on the first look");
  mod = new Notifications();
  mod.init({}, { spawn: () => {} });
  expect(look("done")).toEqual([]);
  expect(look("done", { active: true })).toEqual(["withdraw agent:t1"]);
});
