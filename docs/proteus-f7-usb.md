# Proteus F7 Virtual USB CDC

The emulator models enough of the STM32F767 OTG-FS device controller for the
unmodified Proteus firmware's real USB CDC stack to run, then bridges CDC
bulk traffic to one local TCP client. There is no physical USB device, no
Windows USB driver, and no Windows COM port involved — this is CDC-over-TCP.

## Launch

```
cd proteus_f7
cargo run --release --bin stm32-emulator -- config.yaml -v
```

`config.yaml`'s `devices.usb_cdc_tcp` entry opens a listener on
`127.0.0.1:29000` as soon as the emulator starts, independent of when
firmware itself gets around to configuring OTG-FS.

## Connecting

Connect with any raw TCP client — this is not a serial port:

```
ncat 127.0.0.1 29000
```

A Windows COM port or serial terminal will not work here; there is no COM
device to open. Bytes written to the socket become CDC bulk OUT data once
firmware has enumerated and configured the USB interface; CDC bulk IN bytes
firmware sends are written back to the socket unmodified, with no framing
beyond the payload itself.

## One-client rule and disconnects

Only one client may be connected at a time. A second connection attempt
while one client is active is accepted then immediately dropped. If the
active client disconnects, the listener stays open and accepts the next
connection; USB enumeration state is unaffected by TCP disconnects.

## Verified

Connecting one TCP client and sending a byte produces exactly this in the
emulator's log (`-v`):

```
[clk=145620993 pc=0x002793e0] INFO  USB CDC TCP client connected from 127.0.0.1:55343
[clk=145620993 pc=0x002793e0] INFO  Virtual USB host attached
[clk=218761551 pc=0x00234a90] INFO  USB CDC TCP client disconnected
```

"Virtual USB host attached" confirms the listener accepted the connection
and the modeled OTG-FS controller raised a bus reset toward firmware.

## Enumeration confirmed end to end

A single live capture confirms, byte-for-byte, that the virtual host's
`GET_DESCRIPTOR` → `SET_ADDRESS` → `SET_CONFIGURATION` → `SET_LINE_CODING`
sequence drives the real, unmodified firmware through actual enumeration:
firmware pushes the real 18-byte USB device descriptor
(vendor/product/class bytes matching `usbcfg.cpp` exactly), accepts address
5, and — on `SET_CONFIGURATION` — ChibiOS's own `USB_EVENT_CONFIGURED`
callback activates the real bulk endpoint (2, both directions) and
interrupt endpoint (3) with the exact register values ChibiOS's endpoint
descriptors specify, then arms bulk OUT for reception. See
`proteus_f7/usb_trace_notes.md`'s "Full enumeration confirmed end to end
against real firmware" for the complete byte-level trace.

Getting here took three real, evidence-driven bug fixes to the OTG-FS
model — a `GET_DESCRIPTOR` interrupt-timing bug (`DOEPINT.STUP` fired
before firmware could read the SETUP bytes, producing a spurious zero-byte
response instead of the real 18-byte descriptor) and a missing
`DIEPINT.TXFE`/`DTXFSTS` implementation (firmware waited forever for a "TX
FIFO empty" interrupt this project never raised) — both found by reading
ChibiOS's actual USB driver source and confirmed against live captures; see
usb_trace_notes.md for the full account.

## Current limitation

The capture that confirmed the above ran out of its 60-second window during
`SET_LINE_CODING`'s data stage, before `SET_CONTROL_LINE_STATE` and
"configured" state were reached — not a known bug, just an unfinished
capture. A real TunerStudio-style protocol exchange over the bulk endpoint
has not yet been attempted.
