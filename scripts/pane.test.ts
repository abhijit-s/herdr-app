import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The pane header's right-click, without a window: what it opens.
class Element {
  nodes: any[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "children") target.nodes.push(...args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
const element = () => new Element();
const ran: any[] = [];
const program = { key: "claude", name: "Claude Code", agent: true };
let pane: any;
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, image: element, h_flex: element,
  panes: () => [pane], props: () => ({ pane: "p1" }), run: (...args: any[]) => ran.push(args), home: () => "/home/me",
  theme: () => new Proxy({}, { get: () => "#000000" }), settings: () => ({ working_spinner: null }),
  fitHeaderInfo: (segments: any[]) => segments, headerTextWidth: (text: string, font: number) => [...text].length * font * 0.6,
};
const source = await readFile(new URL("../defaults/mods/pane/main.js", import.meta.url), "utf8");
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap((m) => m[1].split(",").map((n) => n.trim()));
const Pane = new Function(...names, source.replace(/^import .*;\n/gm, "").replace("export default class Pane", "return class Pane"))(...names.map((n) => values[n]));

// Herdr's pane.menu opens the layout's terminal_menu for the pane, its terminal and folder, nothing selected.
test("a right-click on a pane header asks Herdr for that pane's menu", () => {
  for (const [terminal, cwd] of [["t1", "/home/me/src"], [null, null]]) {
    pane = { pane: "p1", terminal, label: "build", program, status: null, cwd };
    ran.length = 0;
    const view = new Pane();
    view.init();
    const header = view.render({});
    const [button, handler] = header.calls.on_mouse_down;
    let stopped = false;
    handler({}, { stop_propagation: () => { stopped = true; } });
    expect(button).toBe("right");
    expect(stopped).toBe(true);
    expect(ran).toEqual([["pane.menu", { pane: "p1" }]]);
  }
});
