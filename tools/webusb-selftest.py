#!/usr/bin/env python3
"""Exercise the web editor's WebUSB paths without hardware.

The editor has no other automated coverage, and the two bugs this caught were
both invisible to reading the code and to a plain "does it render" check:

  1. connect() wrapped itself in withBusy() while the click handler also
     wrapped it, so the guard's `if (busy) return` fired and requestDevice()
     was never reached. Clicking Connect did nothing at all.

  2. `device.addEventListener("disconnect", ...)` -- USBDevice is not an
     EventTarget, so that throws TypeError and aborts connect.

The harness therefore stubs WebUSB with the REAL API shape, including the
absence of addEventListener on the device. A stub that is more permissive
than the real thing is worse than no stub: the first version of this harness
gave the fake device a no-op addEventListener and happily passed while the
page was broken in the browser.

Usage:  just webtest        (or)  python3 tools/webusb-selftest.py
Needs a Chromium-family browser; Brave, Chrome and Chromium are looked for.
"""

import http.server
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "web"

BROWSERS = [
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
]

HARNESS = """
<script>
/* Faithful WebUSB stub. Mirrors the real API shape, including what is NOT
   there: USBDevice has no addEventListener -- disconnect events come from
   navigator.usb. Keep it faithful or it will mask bugs. */
window.__calls = [];
window.__fail = [];

const FACTORY = [0xc0, 0x01,
  0x5f, 0x60, 0x61, 0x54, 0x5c, 0x5d, 0x5e, 0x55,
  0x59, 0x5a, 0x5b, 0x56, 0x62, 0x63, 0x58, 0x57,
  0x08, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x82];

class FakeUSBDevice {
  // Deliberately no addEventListener / removeEventListener, matching USBDevice.
  constructor() { this.productName = "pico-numpad"; this._pending = null; }
  async open() { window.__calls.push("open"); }
  async selectConfiguration(n) { window.__calls.push("selectConfiguration:" + n); }
  async claimInterface(i) { window.__calls.push("claimInterface:" + i); }
  async releaseInterface(i) { window.__calls.push("releaseInterface:" + i); }
  async close() { window.__calls.push("close"); }
  async transferOut(ep, bytes) {
    const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    window.__calls.push("out:0x" + b[0].toString(16));
    if (b[0] === 0x01) this._pending = new Uint8Array([0x00, ...FACTORY]);
    else this._pending = new Uint8Array([0x00, b[0]]);
  }
  async transferIn(ep, len) {
    const out = this._pending || new Uint8Array([0x00, 0x00]);
    this._pending = null;
    return { status: "ok", data: new DataView(out.buffer, out.byteOffset, out.byteLength) };
  }
}

const usbTarget = new EventTarget();
usbTarget._dev = null;
usbTarget.requestDevice = async function (opts) {
  window.__calls.push("requestDevice");
  window.__filter = opts;
  this._dev = new FakeUSBDevice();
  return this._dev;
};
Object.defineProperty(navigator, "usb", { configurable: true, value: usbTarget });

const log = (obj) => {
  const d = document.createElement("div");
  d.id = "TESTRESULT";
  d.textContent = JSON.stringify(obj);
  document.body.appendChild(d);
};

window.addEventListener("error", (e) => window.__fail.push(String(e.message)));

window.addEventListener("load", async () => {
  const r = [];
  const chip = () => document.getElementById("chipText").textContent;
  const wait = (ms) => new Promise((res) => setTimeout(res, ms));

  // The stub must match reality: a USBDevice is not an EventTarget.
  r.push(["stub is faithful: USBDevice has no addEventListener",
          typeof new FakeUSBDevice().addEventListener === "undefined"]);

  document.getElementById("connect").click();
  await wait(500);

  r.push(["requestDevice called exactly once",
          window.__calls.filter((c) => c === "requestDevice").length === 1]);
  r.push(["filter is the pad's VID:PID",
          JSON.stringify(window.__filter) ===
            JSON.stringify({ filters: [{ vendorId: 0x2e8a, productId: 0x000a }] })]);
  r.push(["no TypeError during connect", window.__fail.length === 0]);
  r.push(["opened, configured and claimed",
          ["open", "selectConfiguration:1", "claimInterface:1"].every((c) =>
            window.__calls.includes(c))]);
  r.push(["GET_CONFIG sent", window.__calls.includes("out:0x1")]);
  r.push(["chip reports connected", /Connected/.test(chip())]);
  r.push(["grid loaded from device (KP 7 on key 1)",
          document.getElementById("pad").textContent.includes("KP 7")]);

  // --- consumer / media keys ---
  document.getElementById("modeMedia").click();
  await wait(60);
  r.push(["media mode lists consumer keys",
          document.getElementById("groups").textContent.includes("Volume Up")]);

  document.getElementById("pad").children[0].click();
  await wait(60);
  const vol = [...document.querySelectorAll("#groups button.pick")]
    .find((b) => b.textContent.includes("Volume Up"));
  if (vol) vol.click();
  await wait(60);
  const k0 = document.getElementById("pad").children[0];
  r.push(["media key shows its consumer name on the pad",
          k0.textContent.includes("Volume Up")]);
  r.push(["media key carries the media tag", !!k0.querySelector(".tag")]);
  r.push(["consumer mask bit 0 set and code 0xE9 in keymap",
          (draft[20] & 0x01) === 0x01 && draft[2] === 0xe9]);

  // Reassigning from the keyboard list must clear the mask, otherwise the key
  // would keep firing on the consumer page.
  document.getElementById("modeKbd").click();
  await wait(60);
  const kp7 = [...document.querySelectorAll("#groups button.pick")]
    .find((b) => b.textContent.includes("KP 7"));
  if (kp7) kp7.click();
  await wait(60);
  const k0b = document.getElementById("pad").children[0];
  r.push(["keyboard reassign clears the media tag and mask bit",
          !k0b.querySelector(".tag") && draft[2] === 0x5f && (draft[20] & 0x01) === 0]);

  // --- slot colours ---
  // The stub returns a v1 record, which has no colours. The editor must fill in
  // the defaults rather than read the reserved bytes, matching the firmware.
  const DEF_COLORS = [255, 0, 216, 0, 216, 255, 255, 216, 0];
  r.push(["v1 record migrates to default slot colours, not its reserved bytes",
          JSON.stringify([...draft.slice(22, 31)]) === JSON.stringify(DEF_COLORS)]);
  const c0 = document.getElementById("slotColor0");
  r.push(["colour picker exists and reflects the draft",
          !!c0 && c0.value === "#ff00d8"]);
  if (c0) {
    c0.value = "#00ff00";
    c0.dispatchEvent(new Event("input", { bubbles: true }));
    await wait(60);
    r.push(["colour picker writes to its own three bytes only",
            draft[22] === 0x00 && draft[23] === 0xff && draft[24] === 0x00 &&
            draft[25] === 0x00 && draft[26] === 216 && draft[27] === 255]);
  }

  // A disconnect fired by navigator.usb must be reflected in the chip and not
  // be immediately clobbered back to "Not connected" by setControls().
  const ev = new Event("disconnect");
  ev.device = usbTarget._dev;
  usbTarget.dispatchEvent(ev);
  await wait(200);
  r.push(["disconnect event reported", /disconnected/i.test(chip())]);

  log(r);
});
</script>
"""


def find_browser() -> str:
    for b in BROWSERS:
        if os.access(b, os.X_OK):
            return b
    for name in ("chromium", "google-chrome", "chrome"):
        p = shutil.which(name)
        if p:
            return p
    sys.exit("no Chromium-family browser found (looked for Brave, Chrome, Chromium)")


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def check_media_key_lists_agree() -> None:
    """The firmware's consumer descriptor is generated from CONSUMER_KEYS, so a
    media key the editor offers that is not in that list has no bit in the
    report and silently does nothing. Nothing in the type system or the build
    connects the Rust table to the JavaScript one, so check it here."""
    hid = (ROOT / "firmware" / "src" / "hid.rs").read_text()
    m = re.search(r"pub const CONSUMER_KEYS: \[u8; (\d+)\] = \[(.*?)\n\];", hid, re.S)
    if not m:
        sys.exit("could not find CONSUMER_KEYS in firmware/src/hid.rs")
    declared_len = int(m.group(1))
    body = re.sub(r"//[^\n]*", "", m.group(2))
    fw = [int(x, 16) for x in re.findall(r"0x([0-9a-fA-F]{2})", body)]
    if len(fw) != declared_len:
        sys.exit(f"CONSUMER_KEYS declares {declared_len} entries but has {len(fw)}")
    if len(set(fw)) != len(fw):
        sys.exit("CONSUMER_KEYS has duplicate entries")

    app = (ROOT / "web" / "app.js").read_text()
    m2 = re.search(r"const MEDIA_KEYS = \[(.*?)\n\];", app, re.S)
    if not m2:
        sys.exit("could not find MEDIA_KEYS in web/app.js")
    web = [int(x, 16) for x in re.findall(r"\[0x([0-9a-fA-F]{2})", m2.group(1))]

    only_fw = sorted(f"0x{c:02x}" for c in set(fw) - set(web))
    only_web = sorted(f"0x{c:02x}" for c in set(web) - set(fw))
    if only_fw or only_web:
        sys.exit(
            "media key lists disagree -- firmware-only=%s editor-only=%s. "
            "A key the editor offers but the firmware does not declare cannot "
            "work; a key declared but not offered is dead weight."
            % (only_fw, only_web)
        )
    print(f"  PASS  firmware and editor offer the same {len(set(fw))} media keys")


def check_slot_color_defaults_agree() -> None:
    """The editor seeds its own factory defaults and substitutes them when
    migrating an older record. If those differ from the firmware's, a device
    reset by firmware and one reset by the editor end up a different colour and
    nothing catches it."""
    hid = (ROOT / "firmware" / "src" / "config.rs").read_text()
    m = re.search(
        r"pub const DEFAULT_SLOT_COLORS: \[\[u8; 3\]; 3\] = \[(.*?)\n\];", hid, re.S
    )
    if not m:
        sys.exit("could not find DEFAULT_SLOT_COLORS in firmware/src/config.rs")
    fw = [
        [int(x) for x in re.findall(r"\b(\d{1,3})\b", line)]
        for line in m.group(1).strip().splitlines()
        if "[" in line
    ]

    app = (ROOT / "web" / "app.js").read_text()
    m2 = re.search(r"const DEFAULT_SLOT_COLORS = \[(.*?)\n\];", app, re.S)
    if not m2:
        sys.exit("could not find DEFAULT_SLOT_COLORS in web/app.js")
    web = [
        [int(x) for x in re.findall(r"\b(\d{1,3})\b", line)]
        for line in m2.group(1).strip().splitlines()
        if "[" in line
    ]

    if len(fw) != 3 or any(len(c) != 3 for c in fw):
        sys.exit(f"firmware DEFAULT_SLOT_COLORS malformed: {fw}")
    if fw != web:
        sys.exit(f"slot colour defaults differ: firmware={fw} editor={web}")
    print("  PASS  firmware and editor share the same slot colour defaults")


def main() -> int:
    check_media_key_lists_agree()
    check_slot_color_defaults_agree()
    browser = find_browser()

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        index = (WEB / "index.html").read_text()
        if '<script src="app.js"></script>' not in index:
            sys.exit("index.html no longer loads app.js as expected; update the harness")
        (tmp / "index.html").write_text(
            index.replace(
                '<script src="app.js"></script>', HARNESS + '\n    <script src="app.js"></script>'
            )
        )
        for asset in WEB.iterdir():
            # Skip index.html: copying it here would clobber the patched copy
            # written above and the harness would silently never run.
            if asset.is_file() and asset.name != "index.html":
                shutil.copy(asset, tmp / asset.name)

        handler = lambda *a, **kw: http.server.SimpleHTTPRequestHandler(*a, directory=str(tmp), **kw)
        httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        port = httpd.server_address[1]
        threading.Thread(target=httpd.serve_forever, daemon=True).start()

        out = subprocess.run(
            [
                browser,
                "--headless",
                "--disable-gpu",
                "--virtual-time-budget=6000",
                "--dump-dom",
                f"http://127.0.0.1:{port}/",
            ],
            capture_output=True,
            text=True,
        )
        httpd.shutdown()

    marker = '<div id="TESTRESULT">'
    i = out.stdout.find(marker)
    if i < 0:
        print("no test result produced; browser output:\n", out.stdout[-2000:])
        return 1
    results = json.loads(out.stdout[i + len(marker):].split("</div>")[0])

    failed = 0
    for name, ok in results:
        print(f"  {'PASS' if ok else 'FAIL'}  {name}")
        failed += 0 if ok else 1
    print(f"\n{len(results) - failed}/{len(results)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
