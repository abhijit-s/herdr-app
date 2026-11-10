#!/usr/bin/env python3
"""Does a Kitty graphics `t=f` command naming a FIFO freeze Herdr's UI thread?

    python3 -I scripts/kitty_fifo_freeze.py --bin target/release/herdr-app --target fifo|png [--label name] [--out dir]

Isolation mirrors scripts/bench.py: its own HERDR_APP_HOME, XDG_CONFIG_HOME,
XDG_STATE_HOME and HC_SESSION (so its own core, socket and holder under this
run's folder), HC_* and XDG_RUNTIME_DIR removed, window opened in the
background. Everything it starts is stopped and its folder removed at the end;
the JSON result (and a `sample` of a frozen app on macOS) is written to --out,
the current folder by default.

Fixed: after the emit the app answers, the FIFO's reply is an error and no
reader is left waiting in open(); a PNG replies OK.
"""

import argparse
import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

RED_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="

# Runs inside the Herdr terminal: sends the APC, collects the terminal's reply
# (raw mode, up to 4 s), records it to a file, then idles.
EMITTER = r"""
import base64, os, select, sys, termios, time, tty
target, out = sys.argv[1], sys.argv[2]
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
tty.setraw(fd)
payload = base64.b64encode(target.encode()).decode()
os.write(1, b"EMIT-START\r\n")
sent = time.time()
os.write(1, f"\033_Ga=t,t=f,f=100,i=7,q=0;{payload}\033\\".encode())
reply = b""
deadline = time.time() + 4
while time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.1)
    if r:
        reply += os.read(fd, 4096)
        if reply.endswith(b"\033\\"):
            break
termios.tcsetattr(fd, termios.TCSADRAIN, old)
with open(out, "w") as f:
    f.write(repr({"sent": sent, "reply": reply.decode("latin-1"), "after_s": round(time.time() - sent, 3)}))
os.write(1, b"\r\nEMIT-MARKER-DONE\r\n")
while True:
    time.sleep(60)
"""


class Run:
    def __init__(self, bin, root, session):
        self.bin, self.root, self.session = bin, root, session

    def env(self):
        e = dict(os.environ)
        for key in ("HC_SOCKET", "HC_TERMINAL_ID", "HC_TERMINAL_SOCKET", "HC_SCOPE", "XDG_RUNTIME_DIR"):
            e.pop(key, None)
        e.update(
            HERDR_APP_HOME=f"{self.root}/home",
            HC_SESSION=self.session,
            XDG_STATE_HOME=f"{self.root}/s",
            XDG_CONFIG_HOME=f"{self.root}/config",
            PYTHONIOENCODING="utf-8",
            HERDR_APP_NO_SOUND="1",
            HERDR_APP_BACKGROUND="1",
            HERDR_BENCH_DRAW_OCCLUDED="1",
            HERDR_APP_BENCH=f"{self.root}/frames.jsonl",
        )
        return e

    def app(self, args, timeout):
        """One control-socket request to the app, timed; never raises on a hang."""
        start = time.monotonic()
        try:
            out = subprocess.run([self.bin] + args, env=self.env(), capture_output=True, text=True, timeout=timeout)
            reply = (out.stdout + out.stderr).strip()
            code = out.returncode
        except subprocess.TimeoutExpired:
            reply, code = f"client timeout after {timeout}s", None
        return {"args": args[0], "code": code, "reply": reply[:300], "seconds": round(time.monotonic() - start, 3)}

    def hc(self, *args, timeout=10):
        try:
            return subprocess.run(["hc", *args], env=self.env(), capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return None


def probe(run, label, log):
    p = run.app(["status"], timeout=9)
    p["at"] = label
    responsive = p["code"] == 0 and "did not answer" not in p["reply"] and "timeout" not in p["reply"]
    p["responsive"] = responsive
    log.append(p)
    print(f"  probe {label:22} responsive={responsive} {p['seconds']:6.3f}s code={p['code']} {p['reply'][:80]!r}", flush=True)
    return responsive


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--target", choices=["fifo", "png"], required=True)
    ap.add_argument("--label", default=None)
    ap.add_argument("--out", default=".")
    a = ap.parse_args()
    bin = os.path.abspath(a.bin)
    label = a.label or a.target
    out_dir = os.path.abspath(a.out)
    # Short and outside $TMPDIR: the core's socket lives under it (macOS socket-path limit).
    root = tempfile.mkdtemp(prefix="k", dir="/var/tmp")
    session = "k" + "".join(c if c.isalnum() else "x" for c in os.path.basename(root)[-3:])
    run = Run(bin, root, session)
    result = {"label": label, "bin": bin, "target_kind": a.target, "root": root, "session": session, "probes": []}
    print(f"run {label}: root={root} session={session}", flush=True)

    os.makedirs(f"{root}/home")
    with open(f"{root}/home/shared.json", "w") as f:
        json.dump({"frame.welcomed": True, "layout": "main"}, f)
    with open(f"{root}/home/settings.json", "w") as f:
        json.dump({"zoom": 1.0}, f)
    with open(f"{root}/emit.py", "w") as f:
        f.write(EMITTER)
    target = f"{root}/img.fifo" if a.target == "fifo" else f"{root}/img.png"
    if a.target == "fifo":
        os.mkfifo(target)
    else:
        with open(target, "wb") as f:
            f.write(base64.b64decode(RED_PNG))
    result["target"] = target
    reply_file = f"{root}/reply.txt"

    log = open(f"{root}/app.log", "w")
    app = awake = None
    try:
        core = run.hc("server", "start", timeout=30)
        if core is None or core.returncode:
            raise SystemExit(f"core startup failed: {core and core.stdout}{core and core.stderr}")
        status = json.loads(run.hc("server", "status", "--json").stdout or "{}")
        result["core"] = {"pid": status.get("pid"), "holders": [h.get("pid") for h in status.get("holders", [])], "socket": status.get("socket")}
        print(f"  core: {result['core']}", flush=True)
        app = subprocess.Popen([bin], env=run.env(), stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        result["app_pid"] = app.pid
        print(f"  app pid {app.pid}", flush=True)
        if shutil.which("caffeinate"):
            awake = subprocess.Popen(["caffeinate", "-d", "-w", str(app.pid)])
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if run.app(["status"], timeout=3)["code"] == 0:
                break
            time.sleep(0.3)
        else:
            raise SystemExit("app didn't come up")
        probe(run, "before", result["probes"])

        argv = [sys.executable, "-I", "-u", f"{root}/emit.py", target, reply_file]
        created = None
        for _ in range(50):
            created = run.app(["do", "space.new", json.dumps({"name": "kitty", "folder": root, "argv": argv})], timeout=15)
            if created["code"] == 0 or "unavailable" not in created["reply"]:
                break
            time.sleep(0.2)
        result["space_new"] = created
        emitted_at = time.monotonic()
        print(f"  space.new: {created}", flush=True)

        time.sleep(2)
        frozen_seen = False
        for i in range(3):
            if not probe(run, f"after-emit-{i}", result["probes"]):
                frozen_seen = True
        result["seconds_since_emit_at_last_probe"] = round(time.monotonic() - emitted_at, 3)

        if frozen_seen and shutil.which("sample"):
            subprocess.run(["sample", str(app.pid), "2", "-file", f"{out_dir}/sample-{label}.txt"], capture_output=True, timeout=30)
            result["sample"] = f"{out_dir}/sample-{label}.txt"

        if a.target == "fifo":
            # A non-blocking writer open succeeds only if some reader is blocked in open().
            t0 = time.monotonic()
            try:
                wfd = os.open(target, os.O_WRONLY | os.O_NONBLOCK)
                os.close(wfd)
                result["unblock"] = {"reader_was_waiting": True, "seconds": round(time.monotonic() - t0, 4)}
            except OSError as e:
                result["unblock"] = {"reader_was_waiting": False, "error": str(e)}
            print(f"  unblock: {result['unblock']}", flush=True)
            time.sleep(1)
            for i in range(2):
                probe(run, f"after-unblock-{i}", result["probes"])

        time.sleep(1)
        try:
            with open(reply_file) as f:
                result["terminal_reply"] = f.read()
        except FileNotFoundError:
            result["terminal_reply"] = None
        print(f"  terminal reply: {result['terminal_reply']}", flush=True)
        ls = run.hc("ls", "--json")
        result["hc_ls"] = (ls.stdout if ls else "")[:2000]
        terms = json.loads(ls.stdout or "{}").get("terminals", []) if ls else []
        if terms:
            rd = run.hc("read", terms[0]["address"], timeout=10)
            screen = rd.stdout if rd else ""
            result["screen_has_marker"] = "EMIT-MARKER-DONE" in screen
            result["screen_tail"] = screen[-600:]
    finally:
        if app and app.poll() is None:
            app.terminate()
            try:
                app.wait(timeout=10)
            except subprocess.TimeoutExpired:
                app.kill()
                app.wait()
        log.close()
        if awake:
            awake.terminate()
            awake.wait()
        ls = run.hc("ls", "--json")
        for t in (json.loads(ls.stdout or "{}").get("terminals", []) if ls else []):
            run.hc("kill", t["address"])
        run.hc("server", "stop", timeout=30)
        try:
            with open(f"{root}/app.log") as f:
                result["app_log_tail"] = f.read()[-3000:]
        except OSError:
            pass
        out = f"{out_dir}/result-{label}.json"
        with open(out, "w") as f:
            json.dump(result, f, indent=1)
        shutil.rmtree(root, ignore_errors=True)
        print(f"result: {out}", flush=True)


if __name__ == "__main__":
    main()
