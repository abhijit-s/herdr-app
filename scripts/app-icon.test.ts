import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

// scripts/app-icon: the icon an install from a checkout writes into the
// macOS bundle or the Linux launcher, from settings.json's `app_icon`.
const ROOT = resolve(import.meta.dir, "..");
const read = (path: string) => readFileSync(join(ROOT, path));

function install(platform: "macos" | "linux", settings: object | string | null, env: Record<string, string> = {}) {
  const home = mkdtempSync(join(tmpdir(), "herdr-icon-home-"));
  if (settings !== null) writeFileSync(join(home, "settings.json"), typeof settings === "string" ? settings : JSON.stringify(settings));
  const dest = mkdtempSync(join(tmpdir(), "herdr-icon-dest-"));
  const run = Bun.spawnSync(["bash", "scripts/app-icon", platform, dest], { cwd: ROOT, env: { ...process.env, HERDR_APP_HOME: home, ...env } });
  return { dest, home, code: run.exitCode, out: run.stdout.toString().trim(), err: run.stderr.toString() };
}
const bundleIcon = (dest: string) => readFileSync(join(dest, "Contents/Resources/Herdr.icns"));

test("the checked-in variant SVGs are what scripts/app-icons.py draws", () => {
  const run = Bun.spawnSync(["python3", "scripts/app-icons.py", "--check"], { cwd: ROOT });
  expect(run.stderr.toString()).toBe("");
  expect(run.exitCode).toBe(0);
});

test("macOS: the chosen built-in lands in the bundle", () => {
  for (const name of ["midnight", "mirage", "glass", "phosphor", "graphite"]) {
    const r = install("macos", { app_icon: name });
    expect(r.code).toBe(0);
    expect(bundleIcon(r.dest).equals(read(`assets/icons/variants/${name}.icns`))).toBe(true);
  }
});

test("macOS: classic, theme, no setting, a broken settings.json and no settings.json all install Herdr.icns", () => {
  for (const settings of [{ app_icon: "classic" }, { app_icon: "theme" }, {}, "{ not json", null]) {
    const r = install("macos", settings);
    expect(r.code).toBe(0);
    expect(r.err).toBe("");
    expect(bundleIcon(r.dest).equals(read("assets/icons/Herdr.icns"))).toBe(true);
  }
});

test("macOS: an .icns file is copied; a missing file or an SVG keeps classic and says so", () => {
  const dir = mkdtempSync(join(tmpdir(), "herdr-icon-file-"));
  const mine = join(dir, "mine.icns");
  writeFileSync(mine, read("assets/icons/variants/glass.icns"));
  expect(bundleIcon(install("macos", { app_icon: mine }).dest).equals(read("assets/icons/variants/glass.icns"))).toBe(true);
  for (const app_icon of [join(dir, "gone.icns"), join(ROOT, "assets/icons/variants/mirage.svg")]) {
    const r = install("macos", { app_icon });
    expect(r.code).toBe(0);
    expect(r.err).toContain("classic");
    expect(bundleIcon(r.dest).equals(read("assets/icons/Herdr.icns"))).toBe(true);
  }
});

test("a ~/ path is the home folder's; a relative path installs classic, as Herdr refuses it", () => {
  const fakeHome = mkdtempSync(join(tmpdir(), "herdr-icon-userhome-"));
  writeFileSync(join(fakeHome, "mine.icns"), read("assets/icons/variants/midnight.icns"));
  const r = install("macos", { app_icon: "~/mine.icns" }, { HOME: fakeHome });
  expect(bundleIcon(r.dest).equals(read("assets/icons/variants/midnight.icns"))).toBe(true);
  const rel = install("macos", { app_icon: "assets/icons/variants/glass.icns" });
  expect(rel.code).toBe(0);
  expect(rel.err).toContain("classic");
  expect(bundleIcon(rel.dest).equals(read("assets/icons/Herdr.icns"))).toBe(true);
});

test.if(process.platform === "darwin")("macOS: a .png that isn't one installs classic and the install goes on", () => {
  const dir = mkdtempSync(join(tmpdir(), "herdr-icon-bad-"));
  writeFileSync(join(dir, "bad.png"), "not a png");
  const r = install("macos", { app_icon: join(dir, "bad.png") });
  expect(r.code).toBe(0);
  expect(r.err).toContain("classic");
  expect(bundleIcon(r.dest).equals(read("assets/icons/Herdr.icns"))).toBe(true);
});

test("without python3 the classic icon installs", () => {
  const bin = mkdtempSync(join(tmpdir(), "herdr-icon-bin-"));
  for (const tool of ["bash", "dirname", "cp", "mkdir", "touch", "realpath"]) {
    const found = Bun.which(tool);
    if (found) Bun.spawnSync(["ln", "-s", found, join(bin, tool)]);
  }
  const r = install("macos", { app_icon: "mirage" }, { PATH: bin });
  expect(r.code).toBe(0);
  expect(r.err).toContain("python3");
  expect(bundleIcon(r.dest).equals(read("assets/icons/Herdr.icns"))).toBe(true);
});

test.if(process.platform === "darwin")("macOS: a .png file is converted to an .icns", () => {
  const r = install("macos", { app_icon: join(ROOT, "defaults/frame/settings/app-icons/mirage.png") });
  expect(r.code).toBe(0);
  expect(bundleIcon(r.dest).subarray(0, 4).toString()).toBe("icns");
});

test("Linux: a built-in's SVG becomes the launcher icon; a PNG file is named by its path", () => {
  const r = install("linux", { app_icon: "phosphor" });
  expect(r.out).toBe("herdr-app");
  expect(readFileSync(join(r.dest, "icons/hicolor/scalable/apps/herdr-app.svg")).equals(read("assets/icons/variants/phosphor.svg"))).toBe(true);
  const classic = install("linux", null);
  expect(readFileSync(join(classic.dest, "icons/hicolor/scalable/apps/herdr-app.svg")).equals(read("assets/icons/herdr-ui-icon-clean.svg"))).toBe(true);
  const png = join(ROOT, "defaults/frame/settings/app-icons/mirage.png");
  const file = install("linux", { app_icon: png });
  expect(file.out).toBe(png);
  expect(existsSync(join(file.dest, "icons/hicolor/scalable/apps/herdr-app.svg"))).toBe(false);
  const icns = install("linux", { app_icon: join(ROOT, "assets/icons/variants/glass.icns") });
  expect(icns.out).toBe("herdr-app");
  expect(icns.err).toContain("classic");
  expect(readFileSync(join(icns.dest, "icons/hicolor/scalable/apps/herdr-app.svg")).equals(read("assets/icons/herdr-ui-icon-clean.svg"))).toBe(true);
});

test("the install scripts take their icon from scripts/app-icon", () => {
  expect(read("scripts/install-macos").toString()).toContain('scripts/app-icon macos "$app"');
  const linux = read("scripts/install-linux").toString();
  expect(linux).toContain('icon=$(scripts/app-icon linux "$data")');
  expect(linux).toContain("Icon=$icon");
});
