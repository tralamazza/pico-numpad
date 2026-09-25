#!/usr/bin/env python3
"""Round-trip a consumer (media) key through the real device over the vendor
bulk protocol.

This checks the config v2 wire format against the firmware itself rather than
against the editor's own reimplementation of it -- the two could agree with
each other and both disagree with the device.

Backs up the current config first, then leaves the media key set so the key
press can be tried by hand.

    /tmp/usbenv/bin/python tools/media-roundtrip.py
"""
import sys

import usb.core
import usb.util

VID, PID = 0x2E8A, 0x000A
CMD_GET, CMD_SET, CMD_SAVE = 0x01, 0x02, 0x03
OK = 0x00
CONFIG_LEN = 32
KEYMAP_OFF, MASK_OFF, CKSUM_OFF = 2, 20, 31

# Physical key '7' becomes Volume Up.
KEY_INDEX = 0
VOLUME_UP = 0xE9
BACKUP = "/tmp/config-backup.bin"


def cksum(b):
    s = 0
    for x in b[: CONFIG_LEN - 1]:
        s = (s + x) & 0xFF
    return s


def get_config(ep_in, ep_out):
    ep_out.write(bytes([CMD_GET]))
    # Ask for a full packet rather than exactly 33: the macOS Darwin libusb
    # backend reports LIBUSB_ERROR_OVERFLOW when the requested size is not a
    # whole max-packet multiple, even though the device sends 33 bytes.
    r = bytes(ep_in.read(64, 2000))
    if len(r) < 1 + CONFIG_LEN or r[0] != OK:
        raise SystemExit(f"GET_CONFIG failed: {list(r[:4])}")
    return r[1 : 1 + CONFIG_LEN]


def main():
    dev = usb.core.find(idVendor=VID, idProduct=PID)
    if dev is None:
        raise SystemExit("pico-numpad not found on USB")

    cfg = dev.get_active_configuration()
    # Interface 1 is the vendor config channel (bulk IN 0x81 / OUT 0x01).
    # Interface 0 is the WebUSB landing-page capability and has no endpoints;
    # interface 2 is the HID interface on an interrupt endpoint.
    intf = cfg[(1, 0)]
    if intf.bInterfaceNumber != 1:
        raise SystemExit(f"expected the config channel on interface 1, got {intf.bInterfaceNumber}")
    ep_out = usb.util.find_descriptor(
        intf,
        custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress)
        == usb.util.ENDPOINT_OUT,
    )
    ep_in = usb.util.find_descriptor(
        intf,
        custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress)
        == usb.util.ENDPOINT_IN,
    )
    if not (ep_in and ep_out):
        raise SystemExit("no bulk IN/OUT pair on interface 0")
    print(f"iface={intf.bInterfaceNumber} "
          f"in=0x{ep_in.bEndpointAddress:02x} out=0x{ep_out.bEndpointAddress:02x}")

    # A previous run that died mid-transfer leaves a pending response and flips
    # the data toggle, which shows up as LIBUSB_ERROR_OVERFLOW on the next read.
    # Resetting gets both back to a known state.
    try:
        dev.reset()
        dev.set_configuration()
    except usb.core.USBError as e:
        print(f"(reset skipped: {e})")

    usb.util.claim_interface(dev, intf.bInterfaceNumber)
    try:
        before = get_config(ep_in, ep_out)
        open(BACKUP, "wb").write(before)
        print(f"before: ver={before[1]} "
              f"mask=0x{int.from_bytes(before[MASK_OFF:MASK_OFF + 2], 'little'):04x}")

        new = bytearray(before)
        new[1] = 2
        new[KEYMAP_OFF + KEY_INDEX] = VOLUME_UP
        new[MASK_OFF] |= 0x01
        new[CKSUM_OFF] = cksum(new)

        ep_out.write(bytes([CMD_SET]) + bytes(new))
        st = bytes(ep_in.read(64, 2000))
        print(f"SET  -> {st[0]:#04x} {'OK' if st[0] == OK else 'REJECTED'}")
        if st[0] != OK:
            raise SystemExit("firmware rejected the v2 record with a media mask")

        ep_out.write(bytes([CMD_SAVE]))
        st = bytes(ep_in.read(64, 2000))
        print(f"SAVE -> {st[0]:#04x} {'OK' if st[0] == OK else 'FAILED'}")

        after = get_config(ep_in, ep_out)
        print(f"after:  ver={after[1]} key{KEY_INDEX}=0x{after[KEYMAP_OFF + KEY_INDEX]:02x} "
              f"mask=0x{int.from_bytes(after[MASK_OFF:MASK_OFF + 2], 'little'):04x}")

        untouched = all(
            after[i] == before[i]
            for i in range(2, CONFIG_LEN)
            if i not in (KEYMAP_OFF + KEY_INDEX, MASK_OFF, CKSUM_OFF)
        )
        checks = [
            ("version is 2", after[1] == 2),
            ("media code stored", after[KEYMAP_OFF + KEY_INDEX] == VOLUME_UP),
            ("mask bit set", (after[MASK_OFF] & 1) == 1),
            ("checksum valid", after[CKSUM_OFF] == cksum(after)),
            ("brightness/led/other keys untouched", untouched),
            ("save persisted", after == new),
        ]
        for name, ok in checks:
            print(f"  {'PASS' if ok else 'FAIL'}  {name}")
        good = all(ok for _, ok in checks)
        print(f"\n{sum(ok for _, ok in checks)}/{len(checks)} passed")
        if good:
            print("\nKey '7' now emits Volume Up -- press it to test for real.")
            print(f"Prior config is at {BACKUP}")
    finally:
        usb.util.release_interface(dev, intf.bInterfaceNumber)
        usb.util.dispose_resources(dev)
    return 0 if good else 1


if __name__ == "__main__":
    sys.exit(main())
