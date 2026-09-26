# Web Bluetooth config channel — findings (not built)

Goal: let the web editor talk to the pad over BLE instead of requiring a cable.

## Resolution: two explicit modes, not concurrent connections

The concurrency blocker is avoided rather than solved. The pad gets two
advertised modes, switched by a deliberate physical gesture:

- **HID mode** (default) — HOGP keyboard peripheral, what it does today.
- **Config mode** — stops HOGP advertising, advertises a vendor config service
  that Web Bluetooth can reach.

This is better than concurrent connections on three counts:

1. No second simultaneous link, so no controller-limit gamble and no
   advertising-while-connected radio cost on a battery-powered device.
2. Config over the air is only available when the user physically put the
   device into config mode. That is an attestation the browser cannot forge and
   it answers most of the security objection below.
3. Discoverability is solved by construction: in config mode the pad is
   advertising the thing the web page is looking for.

Cost: configuring requires a deliberate mode switch, and the pad is not a
keyboard while in config mode. For a device you configure occasionally, that is
the right trade.

## Hard constraint: Web Bluetooth cannot touch the HID service

`00001812-...` (Human Interface Device) is on the Web Bluetooth GATT blocklist.
The blocklist entry's own reasoning: "Direct access to HID devices like keyboards
would let web pages become keyloggers."

Consequence: the existing HOGP service is completely off-limits to the browser.
The config protocol cannot be reused over it. A **new vendor GATT service** with a
random 128-bit UUID is mandatory, with its own characteristics carrying
GET / SET / SAVE / RESET, mirroring the WebUSB bulk protocol.

Verified against `WebBluetoothCG/registries` `gatt_blocklist.txt`. Device
Information (`0x180A`) is *not* blocked as a service — only specific
characteristics such as `peripheral_privacy_flag` (0x2A02) and
`reconnection_address` (0x2A03). A random vendor UUID is unaffected.

## The blocker: the pad is not discoverable while connected

`ble.rs` runs a single advertiser and accepts one connection at a time. Once
`advertiser.accept()` returns a link, advertising stops until that link drops.

So a web page can only reach the pad when it is **not connected to any host**.
With three bonded hosts that reconnect on their own, getting into that state is
awkward — the UX becomes "disconnect your laptop from the pad before you edit
its keymap."

This is the crux. Everything else is mechanical.

Options:

- **(a) Concurrent connections.** Keep HOGP connected and accept a second link
  for config. Probably supported at the cyw43 controller level, but the
  firmware's single-advertiser / single-connection-task structure needs real
  rework, and advertising while connected costs radio time on a device whose
  main power lever is turning things off.
- **(b) Configure only when idle.** Much simpler. Acceptable only if "unpair or
  wait for the link to drop" is a tolerable workflow.
- **(c) BLE for discovery and read, USB for writes.** A half-measure that still
  needs the vendor service but avoids making writes available over the air.

## MTU

Default ATT MTU is 23 bytes, so roughly 20 bytes of payload per write. The
config record is 32 bytes. Needs MTU negotiation or chunking with framing and
sequencing. WebUSB's bulk endpoint hides this completely; GATT does not.

## Security gets worse, not better

Over USB the config channel is gated by physical access plus a browser
permission prompt. Over BLE it is in radio range of anyone.

An unauthenticated writable config characteristic means a nearby attacker can
remap the user's keys. Minimum bar is encrypted transport (Just-Works LE Secure
Connections), and realistically a requirement that the peer is bonded. That
interacts with the HOGP bonding already in place — the config link and the HID
link may be different peers with different trust.

Note this is why Power and Sleep were withdrawn from the assignable media keys
in v0.3.0; the same reasoning about what a remote write can do applies harder
here.

## Browser support is not the problem

Chrome 56+ macOS, Chrome 70+ Windows 10, Chrome/Android, ChromeOS, Edge 79+.
Safari and Firefox never. The editor already requires Chrome/Brave for WebUSB, so
this is not a regression — but it buys no new browsers either.

## Recommendation

Decide the concurrency question first, because it determines whether this is a
weekend or a rewrite.

If "configure while disconnected" is acceptable: build option (b). Vendor GATT
service, chunked config, encrypted. Contained.

If it must work while connected: prototype the concurrent-connection piece
*alone* before anything else. If trouble 0.8 / cyw43 cannot hold a HOGP link and
a config link simultaneously with the current single-advertiser structure, the
feature is not available at all and there is no point building the rest.
