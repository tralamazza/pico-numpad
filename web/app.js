"use strict";

/* ------------------------------------------------------------------ *
 * Wire protocol. Mirrors firmware/src/usb.rs and firmware/src/config.rs.
 * The config is a fixed 32-byte little-endian record:
 *   [0]=magic 0xC0  [1]=version  [2..18]=keymap  [18]=brightness
 *   [19]=led_mode  [20..31]=reserved  [31]=checksum
 * Bytes we do not manage are carried through untouched on write, so a newer
 * firmware's reserved fields survive an edit from an older editor.
 * ------------------------------------------------------------------ */
const VENDOR_ID = 0x2e8a;
const PRODUCT_ID = 0x000a;
const IFACE = 1;
// transferIn/transferOut take the endpoint *number*, not the USB address.
// Bulk IN is 0x81 and bulk OUT is 0x01; both are endpoint number 1.
const EP_IN = 1;
const EP_OUT = 1;

const CONFIG_LEN = 32;
const KEYMAP_OFF = 2;
const BRIGHTNESS_OFF = 18;
const LEDMODE_OFF = 19;
const CHECKSUM_OFF = 31;

const CMD_GET = 0x01;
const CMD_SET = 0x02;
const CMD_SAVE = 0x03;
const CMD_RESET = 0x04;
const OK = 0x00;

/* Physical layout of the pad, index == physical key bit. */
const PHYS = ["7", "8", "9", "/", "4", "5", "6", "*", "1", "2", "3", "-", "0", ".", "Enter", "+"];

/* ------------------------------------------------------------------ *
 * HID usage names, usage page 0x07 (Keyboard/Keypad).
 *
 * Verified against the USB HID Usage Tables (v1.21) ch. 10. The previous
 * revision of this editor had this block off by one: it labelled 0x3A as
 * "Caps Lock" (that is F1) and 0x46 as "F12" (that is PrintScreen), so
 * choosing F1 emitted F2. Caps Lock is 0x39.
 *
 * The device's report descriptor declares Usage Minimum 0x00 / Maximum 0xFF
 * on this page, so any of these is emittable; this list is the only thing
 * that limits what the editor offers.
 * ------------------------------------------------------------------ */
const KEY_GROUPS = [
  {
    name: "Numpad",
    keys: [
      [0x53, "Num Lock"], [0x54, "KP /"], [0x55, "KP *"], [0x56, "KP -"],
      [0x57, "KP +"], [0x58, "KP Enter"],
      [0x59, "KP 1"], [0x5a, "KP 2"], [0x5b, "KP 3"],
      [0x5c, "KP 4"], [0x5d, "KP 5"], [0x5e, "KP 6"],
      [0x5f, "KP 7"], [0x60, "KP 8"], [0x61, "KP 9"],
      [0x62, "KP 0"], [0x63, "KP ."],
    ],
  },
  {
    name: "Modifiers",
    keys: [
      [0xe0, "Left Ctrl"], [0xe1, "Left Shift"], [0xe2, "Left Alt"], [0xe3, "Left GUI"],
      [0xe4, "Right Ctrl"], [0xe5, "Right Shift"], [0xe6, "Right Alt"], [0xe7, "Right GUI"],
    ],
  },
  {
    name: "Letters",
    keys: "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("").map((c, i) => [0x04 + i, c]),
  },
  {
    name: "Number row",
    keys: [
      [0x1e, "1"], [0x1f, "2"], [0x20, "3"], [0x21, "4"], [0x22, "5"],
      [0x23, "6"], [0x24, "7"], [0x25, "8"], [0x26, "9"], [0x27, "0"],
    ],
  },
  {
    name: "Punctuation",
    keys: [
      [0x28, "Enter"], [0x29, "Escape"], [0x2a, "Backspace"], [0x2b, "Tab"],
      [0x2c, "Space"], [0x2d, "- / _"], [0x2e, "= / +"], [0x2f, "[ / {"],
      [0x30, "] / }"], [0x31, "\\ / |"], [0x32, "# / ~ (non-US)"],
      [0x33, "; / :"], [0x34, "' / \""], [0x35, "` / ~"],
      [0x36, ", / <"], [0x37, ". / >"], [0x38, "/ / ?"],
    ],
  },
  {
    name: "Function",
    keys: Array.from({ length: 12 }, (_, i) => [0x3a + i, "F" + (i + 1)]),
  },
  {
    name: "Navigation",
    keys: [
      [0x39, "Caps Lock"], [0x46, "Print Screen"], [0x47, "Scroll Lock"], [0x48, "Pause"],
      [0x49, "Insert"], [0x4a, "Home"], [0x4b, "Page Up"],
      [0x4c, "Delete (fwd)"], [0x4d, "End"], [0x4e, "Page Down"],
      [0x4f, "Right Arrow"], [0x50, "Left Arrow"],
      [0x51, "Down Arrow"], [0x52, "Up Arrow"],
    ],
  },
  { name: "Disabled", keys: [[0x00, "None (key does nothing)"]] },
];

const USAGE_NAME = new Map();
for (const g of KEY_GROUPS) for (const [code, name] of g.keys) USAGE_NAME.set(code, name);

/* KeyboardEvent.code -> HID usage, for press-to-assign. */
const CODE_TO_USAGE = (() => {
  const m = {};
  "abcdefghijklmnopqrstuvwxyz".split("").forEach((c, i) => (m["Key" + c.toUpperCase()] = 0x04 + i));
  "123456789".split("").forEach((c, i) => (m["Digit" + c] = 0x1e + i));
  m.Digit0 = 0x27;
  Object.assign(m, {
    Enter: 0x28, Escape: 0x29, Backspace: 0x2a, Tab: 0x2b, Space: 0x2c,
    Minus: 0x2d, Equal: 0x2e, BracketLeft: 0x2f, BracketRight: 0x30,
    Backslash: 0x31, IntlBackslash: 0x31, Semicolon: 0x33, Quote: 0x34,
    Backquote: 0x35, Comma: 0x36, Period: 0x37, Slash: 0x38,
    CapsLock: 0x39, PrintScreen: 0x46, ScrollLock: 0x47, Pause: 0x48,
    Insert: 0x49, Home: 0x4a, PageUp: 0x4b, Delete: 0x4c, End: 0x4d, PageDown: 0x4e,
    ArrowRight: 0x4f, ArrowLeft: 0x50, ArrowDown: 0x51, ArrowUp: 0x52,
    NumpadLock: 0x53, NumpadEqual: 0x67,
    NumpadDivide: 0x54, NumpadMultiply: 0x55, NumpadSubtract: 0x56,
    NumpadAdd: 0x57, NumpadEnter: 0x58, NumpadDecimal: 0x63, NumpadComma: 0x85,
    ControlLeft: 0xe0, ShiftLeft: 0xe1, AltLeft: 0xe2, MetaLeft: 0xe3,
    ControlRight: 0xe4, ShiftRight: 0xe5, AltRight: 0xe6, MetaRight: 0xe7,
  });
  for (let i = 1; i <= 9; i++) m["Numpad" + i] = 0x59 + (i - 1);
  return m;
})();

/* Layouts loadable into the grid. All codes are real page-0x07 usages. */
const PRESETS = {
  factory: {
    label: "Factory numpad",
    keymap: [0x5f, 0x60, 0x61, 0x54, 0x5c, 0x5d, 0x5e, 0x55, 0x59, 0x5a, 0x5b, 0x56, 0x62, 0x63, 0x58, 0x57],
  },
  numberrow: {
    label: "Number row (for laptops without a numpad)",
    keymap: [0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2a, 0x2c, 0x2b, 0x29, 0x2d],
  },
  navigation: {
    label: "Navigation cluster (arrows, Home/End, PgUp/PgDn)",
    keymap: [0x50, 0x52, 0x51, 0x4f, 0x4a, 0x4d, 0x4b, 0x4e, 0x49, 0x4c, 0x29, 0x2b, 0x28, 0x2a, 0x2c, 0x2d],
  },
  functionrow: {
    label: "Function row (F1–F12 + Esc and modifiers)",
    keymap: [0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x29, 0xe0, 0xe1, 0xe2],
  },
};

/* ------------------------------------------------------------------ *
 * State
 * ------------------------------------------------------------------ */
/* Factory defaults, mirroring firmware Config::default(). Seeding the working
 * copy with this means the grid shows a real layout before you connect instead
 * of sixteen "None" keys, which would read as a disabled device rather than an
 * unloaded one. Overwritten by the device's actual config on connect. */
function factoryBytes() {
  const b = new Uint8Array(CONFIG_LEN);
  b[0] = 0xc0; // magic
  b[1] = 1; // version
  PRESETS.factory.keymap.forEach((u, i) => (b[KEYMAP_OFF + i] = u));
  b[BRIGHTNESS_OFF] = 8;
  b[LEDMODE_OFF] = 1; // HIGHLIGHT
  b[CHECKSUM_OFF] = checksum(b);
  return b;
}

let device = null;
let raw = factoryBytes();   // last state read from / written to the device
let draft = factoryBytes(); // working copy
let selected = null;                    // physical key bit being edited
let capturing = false;
let busy = false;

const $ = (id) => document.getElementById(id);

/* ------------------------------------------------------------------ *
 * Toasts
 * ------------------------------------------------------------------ */
function toast(msg, kind = "", ms = 3200) {
  const el = document.createElement("div");
  el.className = "toast " + kind;
  el.textContent = msg;
  $("toasts").appendChild(el);
  setTimeout(() => el.remove(), ms);
  return el;
}

function setChip(state, text) {
  const chip = $("chip");
  chip.className = "chip" + (state ? " " + state : "");
  $("chipText").textContent = text;
}

/* ------------------------------------------------------------------ *
 * Config helpers
 * ------------------------------------------------------------------ */
function checksum(bytes) {
  let s = 0;
  for (let i = 0; i < CHECKSUM_OFF; i++) s = (s + bytes[i]) & 0xff;
  return s;
}

// factoryBytes() runs at declaration time above, so checksum must exist by then.
// Function declarations hoist, so it does.

function nameFor(code) {
  return USAGE_NAME.get(code) ?? `0x${code.toString(16).padStart(2, "0")}`;
}

function dirtyKeys() {
  const out = [];
  for (let i = 0; i < 16; i++) if (draft[KEYMAP_OFF + i] !== raw[KEYMAP_OFF + i]) out.push(i);
  return out;
}

function isDirty() {
  for (let i = 0; i < CONFIG_LEN; i++) {
    if (i === CHECKSUM_OFF) continue;
    if (draft[i] !== raw[i]) return true;
  }
  return false;
}

function refreshDirty() {
  const n = dirtyKeys().length;
  const settingsChanged =
    draft[BRIGHTNESS_OFF] !== raw[BRIGHTNESS_OFF] || draft[LEDMODE_OFF] !== raw[LEDMODE_OFF];
  const changed = n > 0 || settingsChanged;
  $("dirtyBadge").classList.toggle("show", changed && !!device);
  const parts = [];
  if (n) parts.push(`${n} key${n === 1 ? "" : "s"}`);
  if (settingsChanged) parts.push("settings");
  $("dirtyText").textContent = "Unsaved: " + parts.join(", ");
  $("save").classList.toggle("dirty", changed && !!device);
  renderPad();
  renderGroups();
}

/* ------------------------------------------------------------------ *
 * Rendering
 * ------------------------------------------------------------------ */
function renderPad() {
  const pad = $("pad");
  const changed = new Set(dirtyKeys());
  pad.innerHTML = "";
  for (let i = 0; i < 16; i++) {
    const code = draft[KEYMAP_OFF + i];
    const cell = document.createElement("button");
    cell.type = "button";
    cell.className =
      "key" + (selected === i ? " sel" : "") + (changed.has(i) ? " changed" : "");
    cell.setAttribute("role", "gridcell");
    cell.setAttribute(
      "aria-label",
      `Physical key ${PHYS[i]}, assigned ${nameFor(code)}. Activate to reassign.`
    );
    cell.innerHTML =
      `<span class="code">0x${code.toString(16).padStart(2, "0")}</span>` +
      `<span class="phys">${PHYS[i]}</span>` +
      `<span class="assign${code === 0 ? " none" : ""}">${nameFor(code)}</span>`;
    cell.addEventListener("click", () => selectKey(i));
    pad.appendChild(cell);
  }
}

function renderGroups() {
  const q = $("search").value.trim().toLowerCase();
  const wrap = $("groups");
  wrap.innerHTML = "";
  let shown = 0;

  for (const group of KEY_GROUPS) {
    const matches = group.keys.filter(([code, name]) => {
      if (!q) return true;
      return (
        name.toLowerCase().includes(q) ||
        `0x${code.toString(16)}`.includes(q) ||
        String(code) === q
      );
    });
    if (!matches.length) continue;

    const h = document.createElement("div");
    h.className = "group";
    h.innerHTML = `<h3>${group.name}</h3>`;
    const row = document.createElement("div");
    row.className = "keys";

    for (const [code, name] of matches) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "pick" + (selected !== null && draft[KEYMAP_OFF + selected] === code ? " active" : "");
      b.innerHTML = `${name} <code>0x${code.toString(16).padStart(2, "0")}</code>`;
      b.addEventListener("click", () => assign(code));
      row.appendChild(b);
      shown++;
    }
    h.appendChild(row);
    wrap.appendChild(h);
  }

  if (!shown) {
    wrap.innerHTML = `<div class="empty">No key matches “${escapeHtml(q)}”.</div>`;
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}

function renderCapture() {
  const el = $("capture");
  if (selected === null) {
    el.className = "capture idle";
    $("captureMsg").textContent = "Select a key on the pad to reassign it.";
  } else if (capturing) {
    el.className = "capture";
    $("captureMsg").innerHTML =
      `Press the key you want for <strong>${PHYS[selected]}</strong> now. <kbd>Esc</kbd> cancels.`;
  } else {
    el.className = "capture idle";
    $("captureMsg").innerHTML =
      `<strong>${PHYS[selected]}</strong> → ${nameFor(draft[KEYMAP_OFF + selected])}. ` +
      `Press it again on your keyboard, or pick from the list.`;
  }
}

function renderSettings() {
  const b = draft[BRIGHTNESS_OFF];
  $("brightness").value = String(Math.min(31, b));
  $("brightnessValue").textContent = `${Math.min(31, b)} / 31`;

  const led = draft[LEDMODE_OFF];
  const sel = $("ledMode");
  if (![...sel.options].some((o) => Number(o.value) === led)) {
    // A stored LED mode with no matching <option> leaves select.value at "",
    // and Number("") is 0 -- a later save would silently rewrite the device's
    // LED mode to "Off". Synthesise the option instead.
    const opt = document.createElement("option");
    opt.value = String(led);
    opt.textContent = `Unknown (${led})`;
    sel.appendChild(opt);
  }
  sel.value = String(led);
}

function renderAll() {
  renderPad();
  renderGroups();
  renderCapture();
  renderSettings();
  refreshDirty();
}

/* ------------------------------------------------------------------ *
 * Selection and assignment
 * ------------------------------------------------------------------ */
function selectKey(i) {
  selected = i;
  capturing = true;
  $("search").focus();
  renderPad();
  renderCapture();
  renderGroups();
}

function assign(code) {
  if (selected === null) {
    toast("Select a key on the pad first.", "warn");
    return;
  }
  const prev = draft[KEYMAP_OFF + selected];
  draft[KEYMAP_OFF + selected] = code & 0xff;
  capturing = false;
  $("search").value = "";
  renderAll();
  if (prev !== code) {
    toast(`${PHYS[selected]} → ${nameFor(code)}. Not saved yet.`);
  }
}

function onKeyDown(e) {
  if (!capturing || selected === null) return;
  // Escape cancels capture rather than assigning Escape; use the list for that.
  if (e.key === "Escape") {
    capturing = false;
    renderCapture();
    e.preventDefault();
    return;
  }
  const usage = CODE_TO_USAGE[e.code];
  if (usage === undefined) {
    toast(`Key "${e.code}" has no HID usage on page 0x07.`, "warn");
    return;
  }
  e.preventDefault();
  assign(usage);
}

/* ------------------------------------------------------------------ *
 * Wire I/O
 * ------------------------------------------------------------------ */
async function send(bytes) {
  await device.transferOut(EP_OUT, bytes);
}

async function recvResponse(expectedCmd = null) {
  const result = await device.transferIn(EP_IN, 1 + CONFIG_LEN);
  if (result.status !== "ok") throw new Error(`transferIn status: ${result.status}`);
  const bytes = new Uint8Array(result.data.buffer, result.data.byteOffset, result.data.byteLength);
  if (bytes.length < 1) throw new Error("empty response from device");
  if (expectedCmd !== null && bytes.length > 1 && bytes[1] !== expectedCmd) {
    throw new Error(`bad response echo: got ${bytes[1]}, expected ${expectedCmd}`);
  }
  return bytes;
}

function loadDraftFrom(bytes) {
  raw = bytes.slice(0, CONFIG_LEN);
  draft = raw.slice(0, CONFIG_LEN);
}

async function loadConfig() {
  await send(new Uint8Array([CMD_GET]));
  const resp = await recvResponse();
  if (resp.length < 1 + CONFIG_LEN || resp[0] !== OK) throw new Error("GET_CONFIG failed");
  loadDraftFrom(resp.subarray(1));
  renderAll();
  toast("Loaded config from device.", "ok", 2000);
}

async function applyConfig() {
  const out = draft.slice(0, CONFIG_LEN);
  out[CHECKSUM_OFF] = checksum(out);
  const packet = new Uint8Array(1 + CONFIG_LEN);
  packet[0] = CMD_SET;
  packet.set(out, 1);
  await send(packet);
  const resp = await recvResponse(CMD_SET);
  if (resp[0] !== OK) throw new Error("SET_CONFIG rejected (bad magic, version or checksum)");
  draft = out;
}

async function saveConfig() {
  await applyConfig();
  await send(new Uint8Array([CMD_SAVE]));
  const resp = await recvResponse(CMD_SAVE);
  if (resp[0] !== OK) throw new Error("SAVE failed — flash write rejected");
  raw = draft.slice(0, CONFIG_LEN);
  refreshDirty();
  toast("Saved to flash.", "ok");
}

async function resetDefaults() {
  await send(new Uint8Array([CMD_RESET]));
  const resp = await recvResponse(CMD_RESET);
  if (resp[0] !== OK) throw new Error("RESET failed");
  await loadConfig();
  toast("Device reset to factory defaults. Not saved — use Save to flash to keep it.", "warn", 5000);
}

/* ------------------------------------------------------------------ *
 * Import / export
 * ------------------------------------------------------------------ */
function exportJson() {
  const out = draft.slice(0, CONFIG_LEN);
  out[CHECKSUM_OFF] = checksum(out);
  const profile = {
    kind: "pico-numpad-profile",
    version: 1,
    exported: new Date().toISOString(),
    brightness: draft[BRIGHTNESS_OFF],
    ledMode: draft[LEDMODE_OFF],
    keymap: Array.from({ length: 16 }, (_, i) => ({
      physical: PHYS[i],
      usage: draft[KEYMAP_OFF + i],
      name: nameFor(draft[KEYMAP_OFF + i]),
    })),
    raw: Array.from(out),
  };
  const blob = new Blob([JSON.stringify(profile, null, 2)], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = `pico-numpad-${new Date().toISOString().slice(0, 10)}.json`;
  a.click();
  URL.revokeObjectURL(a.href);
  toast("Profile exported.", "ok");
}

function importJson(file) {
  const reader = new FileReader();
  reader.onload = () => {
    try {
      const p = JSON.parse(String(reader.result));
      const next = draft.slice(0, CONFIG_LEN);
      if (Array.isArray(p.keymap) && p.keymap.length === 16) {
        p.keymap.forEach((entry, i) => {
          const usage = typeof entry === "object" ? entry.usage : entry;
          if (Number.isInteger(usage) && usage >= 0 && usage <= 0xff) next[KEYMAP_OFF + i] = usage;
        });
      } else if (Array.isArray(p.raw) && p.raw.length === CONFIG_LEN) {
        p.raw.forEach((v, i) => {
          if (Number.isInteger(v) && v >= 0 && v <= 0xff) next[i] = v;
        });
      } else {
        throw new Error("no keymap in file");
      }
      if (Number.isInteger(p.brightness)) next[BRIGHTNESS_OFF] = Math.min(31, p.brightness) & 0xff;
      if (Number.isInteger(p.ledMode)) next[LEDMODE_OFF] = p.ledMode & 0xff;
      draft = next;
      renderAll();
      toast("Profile loaded into the grid. Not saved — use Save to flash.", "warn", 5000);
    } catch (err) {
      toast(`Import failed: ${err.message}`, "err", 5000);
    }
  };
  reader.readAsText(file);
}

/* ------------------------------------------------------------------ *
 * Busy / control state
 * ------------------------------------------------------------------ */
function setControls() {
  const on = !!device && !busy;
  $("connect").disabled = !(!device && !busy);
  $("disconnect").disabled = !on;
  $("reload").disabled = !on;
  $("save").disabled = !on;
  $("exportBtn").disabled = !on;
  $("importBtn").disabled = !on;
  $("defaults").disabled = !on;
  $("brightness").disabled = !on;
  $("ledMode").disabled = !on;
  $("preset").disabled = !on;
  $("search").disabled = !on;
  if (!device) setChip("", "Not connected");
  refreshDirty();
}

async function withBusy(fn) {
  if (busy) return;
  busy = true;
  setControls();
  try {
    await fn();
  } catch (err) {
    toast(String(err && err.message ? err.message : err), "err", 6000);
  } finally {
    busy = false;
    setControls();
  }
}

/* ------------------------------------------------------------------ *
 * Connection
 * ------------------------------------------------------------------ */
async function connect() {
  if (!navigator.usb) {
    toast("WebUSB is not available in this browser. Use Chrome or Edge.", "err", 6000);
    return;
  }
  // No withBusy() here on purpose: the click handler already wraps this call, so
  // wrapping again would hit that guard's `if (busy) return` and bail out before
  // requestDevice() ever ran -- the button would do nothing at all.
  device = await navigator.usb.requestDevice({
    filters: [{ vendorId: VENDOR_ID, productId: PRODUCT_ID }],
  });
  await device.open();
  await device.selectConfiguration(1);
  await device.claimInterface(IFACE);
  device.addEventListener("disconnect", onDeviceGone);
  setChip("on", `Connected · ${device.productName || "pico-numpad"}`);
  setControls();
  await loadConfig();
}

function onDeviceGone() {
  device = null;
  selected = null;
  capturing = false;
  setChip("err", "Device disconnected");
  setControls();
  toast("Device disconnected.", "warn");
}

async function disconnect() {
  if (!device) return;
  const dying = device;
  device = null;
  selected = null;
  capturing = false;
  setControls();
  try {
    // Release the interface and close the handle. Leaving them claimed keeps the
    // device held by this tab, so another origin (or a reload of this one)
    // cannot open it until the tab is closed.
    dying.removeEventListener("disconnect", onDeviceGone);
    await dying.releaseInterface(IFACE);
    await dying.close();
    setChip("", "Not connected");
    toast("Disconnected.", "ok", 2000);
  } catch (err) {
    toast(`Disconnect error: ${err.message}`, "err");
  }
}

/* ------------------------------------------------------------------ *
 * Wire up
 * ------------------------------------------------------------------ */
$("connect").addEventListener("click", () => withBusy(connect));
$("disconnect").addEventListener("click", disconnect);
$("reload").addEventListener("click", () => withBusy(loadConfig));
$("save").addEventListener("click", () => withBusy(saveConfig));
$("defaults").addEventListener("click", () => withBusy(resetDefaults));
$("exportBtn").addEventListener("click", exportJson);
$("importBtn").addEventListener("click", () => $("importFile").click());
$("importFile").addEventListener("change", (e) => {
  if (e.target.files && e.target.files[0]) importJson(e.target.files[0]);
  e.target.value = "";
});

$("brightness").addEventListener("input", (e) => {
  draft[BRIGHTNESS_OFF] = Number(e.target.value) & 0xff;
  $("brightnessValue").textContent = `${e.target.value} / 31`;
  refreshDirty();
});

$("ledMode").addEventListener("change", (e) => {
  draft[LEDMODE_OFF] = Number(e.target.value) & 0xff;
  refreshDirty();
});

const presetSel = $("preset");
for (const [id, p] of Object.entries(PRESETS)) {
  const opt = document.createElement("option");
  opt.value = id;
  opt.textContent = p.label;
  presetSel.appendChild(opt);
}
presetSel.addEventListener("change", (e) => {
  const p = PRESETS[e.target.value];
  if (!p) return;
  p.keymap.forEach((u, i) => (draft[KEYMAP_OFF + i] = u & 0xff));
  renderAll();
  e.target.value = "";
  toast(`“${p.label}” loaded into the grid. Not saved.`, "warn", 4500);
});

$("search").addEventListener("input", renderGroups);
document.addEventListener("keydown", onKeyDown);

window.addEventListener("beforeunload", (e) => {
  if (isDirty()) {
    e.preventDefault();
    e.returnValue = "";
  }
});

if (!window.isSecureContext) {
  setChip("err", "Needs HTTPS or localhost");
  toast("WebUSB requires a secure context (HTTPS or localhost).", "err", 8000);
}

renderAll();
