# Proteus F7 OTG-FS trace evidence

Peripheral SVD name: `OTG_FS_GLOBAL` (base 0x5000_0000). IRQ 67
(`OTG_FS_IRQn` from STM32F767.svd), already wired via
`sys.p.nvic.borrow_mut().set_intr_pending(67)` in `OtgFs::poll`.

`OTG_FS_DEVICE` (base 0x5000_0800), `OTG_FS_HOST` (base 0x5000_0400), and
`OTG_FS_PWRCLK` (base 0x5000_0e00) are separate SVD peripherals but are
NOT separately registered: `Peripherals::modeled_range` widens
`OTG_FS_GLOBAL`'s claimed range to `(0x5000_0000, 0x5000_2000)`, so all
four SVD peripherals' addresses, plus FIFO index 0 (`0x5000_1000`-
`0x5000_1fff`), dispatch to the single `OtgFs` instance. The `offset`
`OtgFs::read`/`write` receives is `addr - 0x5000_0000`, i.e. the
`stm32_otg_t` struct offset from ChibiOS's
`os/hal/ports/STM32/LLD/OTGv1/stm32_otg.h`.

Two SVD register names are wrong; this project uses the struct-offset
names instead:
- offset 0x800 (SVD: "CTL") is `DCFG`.
- offset 0x810 (SVD: "TSIZ") is `DIEPMSK`.

## Observed reset sequence (proteus_f7/usb-plan-evidence.log:10597-13469)

Firmware boot-time OTG-FS init (clk=57044973 onward), before any virtual
host activity:
1. write GUSBCFG(0x00c)=0x40001440
2. write DCFG(0x800)=0x02200003
3. write PCGCCTL(0xe00)=0x00000000
4. write GOTGCTL(0x000)=0x000000c0
5. write GCCFG(0x038)=0x00010000
6. read GRSTCTL(0x010)=0x80000000; write GRSTCTL=0x00000001 (core soft reset, self-clears)
7. write GAHBCFG(0x008)=0x00000000
8. for ep in 0..=5: read ie[ep].DIEPCTL(0x900+ep*0x20)=0; read oe[ep].DOEPCTL(0xb00+ep*0x20)=0; write ie[ep].DIEPINT(0x908+ep*0x20)=0xffffffff; write oe[ep].DOEPINT(0xb08+ep*0x20)=0xffffffff (W1C boilerplate over all 6 endpoint pairs)
9. write DAINTMSK(0x81c)=0x00010001 (enable EP0 IN + EP0 OUT)
10. write DIEPMSK(0x810)=0x00000000; write DOEPMSK(0x814)=0x00000000; write DAINTMSK(0x81c)=0x00000000
11. write GINTMSK(0x018)=0xc0303c08
12. write GINTSTS(0x014)=0xffffffff (clear all, W1C)
13. read GAHBCFG=0; write GAHBCFG=0x00000001 (global interrupt enable)
14. read DCTL(0x804)=0x00000002; write DCTL=0x00000000

## Observed virtual-host-reset response (clk=97910816 onward, after `virtual_host_reset()` sets USB_RESET and TCP client connects)

1. read GINTSTS=0x00001000 (USB_RESET); read GINTMSK=0xc0303c08; write GINTSTS=0x00001000 (W1C ack)
2. write GRSTCTL=0x00000020 (TXFFLSH); read GRSTCTL=0x80000000 (self-cleared)
3. write DIEPEMPMSK(0x834)=0x00000000
4. write DAINTMSK(0x81c)=0x00010001
5. for ep in 0..=5: write ie[ep].DIEPCTL=0x08000000; write oe[ep].DOEPCTL=0x08000000; write ie[ep].DIEPINT=0xffffffff; write oe[ep].DOEPINT=0xffffffff
6. write GRXFSIZ(0x024)=0x00000080 (RX FIFO depth 128 words)
7. write GRSTCTL=0x00000010 (RXFFLSH); read GRSTCTL=0x80000000 (self-cleared)
8. read DCFG=0x02200003; write DCFG=0x02200003 (unchanged)
9. read GINTMSK=0xc0303c08; write GINTMSK=0xc03c3c18 (adds RXFLVL, IEPINT, OEPINT)
10. write DIEPMSK(0x810)=0x00000009 (XFRCM|TOCM); write DOEPMSK(0x814)=0x00000009 (XFRCM|STUPM)
11. write oe[0].DOEPTSIZ(0xb10)=0x60000000 (STUPCNT=3)
12. write oe[0].DOEPCTL(0xb00)=0x10008040 (USBAEP|MPSIZ=64, EP0 OUT active)
13. write ie[0].DIEPTSIZ(0x910)=0x00000000
14. write ie[0].DIEPCTL(0x900)=0x10008040 (USBAEP|MPSIZ=64, EP0 IN active)
15. write DIEPTXF0(0x028)=0x00100080 (TX FIFO0: depth 16 words, start word 0x80)
16. idle: read GINTSTS=0; read GINTMSK=0xc03c3c18; write GINTSTS=0 (no-op)
17. read ie[0].DTXFSTS(0x918)=0x00000000 — firmware is now polling for EP0 IN FIFO space before sending its first response; the trace ends here because no SETUP packet has been delivered yet (Task 4 adds that).

## Enumeration (Task 4 verification)

Task 4 adds: the shared RX FIFO/status queue, and a trigger that queues the
first GET_DESCRIPTOR(DEVICE) SETUP packet the first time firmware writes
`oe[0].DOEPCTL` with `USBAEP` (bit 15) newly set — the real trace-observed
value at that write is `0x1000_8040`, which sets `USBAEP` and part of the MPS
field but NOT `EPENA` (bit 31); control endpoint 0 doesn't use `EPENA` for
receiving SETUP packets the way bulk/interrupt endpoints use it for data,
so the original design (gating on `EPENA`) was wrong and had to be corrected
against this evidence.

Re-running the emulator with a connected TCP client past this point confirms,
byte-for-byte:
- `GRXSTSP` pop returns `0x000c0080` — decodes to `PKTSTS=SETUP_DATA(6)`,
  `BCNT=8`, `EPNUM=0`, exactly matching this project's `rx_status_word`
  encoding for the queued GET_DESCRIPTOR packet.
- `oe[0].DOEPINT` read returns `0x00000008` (`STUP` bit) and firmware's W1C
  write of the same value correctly clears it on the next read.
- Firmware subsequently writes `ie[0].DIEPTSIZ=0x00080000` (`PKTCNT=1`,
  `XFRSIZ=0`) then `ie[0].DIEPCTL=0x9400_8040` (adds `EPENA` and `CNAK` to
  the existing `USBAEP` value) — a legitimate zero-byte IN arm per the DWC2
  register model, which our `complete_in_transfer` correctly completes
  immediately and uses to advance the virtual host to the next stage.
- Zero `peri=????`/unmapped accesses in the OTG-FS address range throughout.

Whether ChibiOS's driver treats this zero-byte completion as the actual
GET_DESCRIPTOR response (vs. a priming/ACK step with the real descriptor
bytes following in a later, not-yet-captured exchange) is not yet confirmed
by a longer capture — verbosity level materially affects how much real
wall-clock time elapses per instruction (see below), which in turn affects
how far any timer-gated firmware code advances within a practical capture
window. This project treats the current behavior as correct per DWC2
register semantics (a real zero-byte completion, not a fabrication) and
defers full end-to-end confirmation to Task 6's manual TunerStudio-style
protocol exchange, which will surface any real sequencing gap with a real
payload instead of more speculative single-shot trace reading.

**Timing note:** `-vvv` (register/peripheral-level trace, no per-instruction
disassembly) runs dramatically faster in real time than `-vvvv` (adds
per-instruction disassembly), and this project observed the two verbosity
levels reach materially different points in firmware execution within the
same `--max-instructions` bound. This suggests some firmware delay path is
gated by a timer or clock source that correlates with real elapsed wall-clock
time rather than purely with retired instruction count — worth keeping in
mind for any future bring-up work that depends on reaching a specific point
in firmware execution within a bounded instruction count.

## Bulk endpoints (Task 5 — resolved from source, not from a live trace)

Task 5's plan called for discovering the real CDC bulk IN/OUT endpoint
numbers from a live trace. Four live-capture attempts (varying connection
timing and hold duration, both `-vvv` and `-vvvv`) did not reach a point
where firmware activates a non-zero endpoint — see the "Live capture
attempts" subsection below for what was tried. The numbers were instead
found directly in the firmware source
(`C:\Users\Tera\Documents\GitHub\epicefi_fw`, the real build tree used
earlier this session to produce `epicefi.elf`), which is authoritative and
didn't require more speculative wall-clock-timed captures:

`firmware/hw_layer/ports/stm32/serial_over_usb/usbcfg.cpp`:
```c
#define USBD1_DATA_REQUEST_EP           2
#define USBD1_DATA_AVAILABLE_EP         2
#define USBD1_INTERRUPT_REQUEST_EP      3
```

CDC bulk IN and OUT both use endpoint **2** (USB allows the same endpoint
number for both directions — DIEPCTL/DOEPCTL are physically separate
registers per endpoint number, so this isn't a conflict). Endpoint 3 is a
CDC interrupt-IN notification endpoint, not bulk, and is not wired to the
TCP bridge. `OtgFs::new` now calls `set_bulk_endpoints(2, 2)` — no config
option, matching the design's non-goals (firmware's endpoint assignment is
fixed at compile time).

Endpoints are only enabled once ChibiOS's own `usb_event` callback receives
`USB_EVENT_CONFIGURED` (`usbInitEndpointI(usbp, USBD1_DATA_REQUEST_EP,
&cdcDataEpConfig)` in the same file), which its `hal_usb.c` fires from the
`SET_CONFIGURATION` request handler — i.e. bulk forwarding only becomes
live once the virtual host's `SET_CONFIGURATION` stage completes.

### Resolved: the zero-byte transfer was a real bug, now fixed

`ChibiOS/os/hal/src/hal_usb.c`'s `_usb_ep0setup` computes the actual
descriptor transfer size from `usbSetupTransfer`'s `n` argument (18 bytes
for the observed `GET_DESCRIPTOR(DEVICE)` request — see
`default_handler`'s `USB_REQ_GET_DESCRIPTOR` case, `usbSetupTransfer(usbp,
dp->ud_string, dp->ud_size, NULL)` where `dp->ud_size == 18`). The
zero-byte `DIEPTSIZ`/`DIEPCTL` arm this project originally observed and
mistakenly treated as the GET_DESCRIPTOR completion matched
`OTGv1/hal_usb_lld.c`'s `usb_lld_start_in`, `isp->txsize == 0` branch
exactly (`0x00080000`) — not the nonzero-size branch that carries the real
18 bytes.

Root cause: `virtual_host_setup` raised `DOEPINT.STUP` and set
`GINTSTS.RXFLVL` simultaneously, synchronously with queuing the packet. But
`hal_usb_lld.c`'s ISR dispatch (`usb_lld_serve_interrupt`) processes OEPINT
(which invokes `_usb_ep0setup`) *before* it processes RXFLVL (which pops
`GRXSTSP` and copies the SETUP bytes into `setup_buf`) in the same pass —
so `_usb_ep0setup` ran with an empty/stale setup buffer, and whatever it
decoded from that triggered the spurious zero-byte transfer instead of the
real 18-byte one.

**Fix** (`src/peripherals/otg_fs.rs`): `DOEPINT.STUP` is no longer raised
in `virtual_host_setup` — it's now raised only when firmware pops the
`SETUP_COMP` status word via `GRXSTSP` (i.e. once the RXFIFO delivery is
actually complete, matching real silicon). `GINTSTS.RXFLVL` is now computed
dynamically from `rx_status`'s non-emptiness rather than stored as a
firmware-clearable bit, since it's read-only/level-triggered on real
hardware (confirmed by the trace showing firmware writing back the RXFLVL
bit as part of a blanket ack, which real hardware would simply ignore). The
same fix was applied to the bulk OUT completion path (`DOEPINT.XFRC` now
also fires on the `OUT_COMP` pop, not eagerly).

**Verified against a fresh live capture**: firmware now reads the actual
FIFO bytes (`0x01000680`, `0x00120000` — decoding to the real
`bmRequestType/bRequest/wValue` and `wIndex/wLength=18` of the SETUP
packet), then writes `ie[0].DIEPTSIZ = 0x2008_0012` (`XFRSIZ=18`, matching
`usb_lld_start_in`'s nonzero-size branch: `MCNT(1)|PKTCNT(1)|XFRSIZ(18)`)
before arming `DIEPCTL` — the real 18-byte descriptor response, not the
previous spurious zero-byte one.

## A third bug, found by the same live capture: DIEPINT.TXFE never fired

Fixing the above got firmware to correctly arm the 18-byte transfer, enable
`DIEPEMPMSK` (TX FIFO empty interrupt) for EP0 IN, and then simply stop —
forever, in every subsequent capture. Cross-referencing
`OTGv1/hal_usb_lld.c`'s `otg_epin_handler`/`otg_txfifo_handler` explained
why: ChibiOS only pushes IN transfer bytes into the FIFO in response to
`DIEPINT.TXFE` firing (gated by `DIEPEMPMSK`), and only if `DTXFSTS` reports
enough free space. This project's model never raised `DIEPINT.TXFE` at all,
and `DTXFSTS` was stubbed to always read `0` (a Task 3 placeholder, "FIFO
space accounting arrives in Task 5" — never actually implemented in Task
5) — so `otg_txfifo_handler`'s space check always failed even if `TXFE` had
fired.

**Fix** (`src/peripherals/otg_fs.rs`): `DTXFSTS` now reports a fixed 16
words (64 bytes — matching the configured `DIEPTXF0` depth, plenty for any
single MPS=64 packet this project's control/CDC transfers ever use; this
project does not model real FIFO exhaustion, which is out of scope). Writing
`DIEPEMPMSK` with a newly-unmasked bit for an endpoint whose `DIEPCTL` still
has `EPENA` set (i.e. an IN transfer is actively armed) now immediately
raises `DIEPINT.TXFE` for that endpoint, matching real silicon's
level-triggered behavior (the FIFO starts empty, so unmasking the interrupt
while a transfer is armed and the FIFO has room should fire immediately).
Zero-byte transfers are unaffected: they complete (clearing `EPENA`) before
`DIEPEMPMSK` is ever written, so the new check correctly does not fire
`TXFE` for them.

## Full enumeration confirmed end to end against real firmware

With all three fixes in place, a single ~60-second live capture (TCP client
connected after boot, held open) shows, in order, byte-for-byte:

1. **`GET_DESCRIPTOR(DEVICE)` response**: firmware pushes exactly
   `[0x12,0x01,0x00,0x02, 0xef,0x02,0x01,0x40, 0x83,0x04,0x40,0x57,
   0x00,0x02,0x01,0x02, 0x03,0x01]` (18 bytes, 5 FIFO words) into `FIFO[0]`
   — this is byte-for-byte the real `vcom_device_descriptor_data` from
   `usbcfg.cpp` (bLength=18, bDescriptorType=DEVICE, bcdUSB=0x0200,
   bDeviceClass=0xEF, idVendor=0x0483, idProduct=0x5740, bcdDevice=0x0200,
   iManufacturer=1, iProduct=2, iSerialNumber=3, bNumConfigurations=1).
2. **`SET_ADDRESS(5)`**: firmware reads back `[0x00,0x05,0x05,0x00,
   0x00,0x00,0x00,0x00]` from the FIFO — exactly this project's
   `set_address_packet(VIRTUAL_DEVICE_ADDRESS=5)`.
3. **`SET_CONFIGURATION(1)`**: firmware reads back
   `[0x00,0x09,0x01,0x00, 0x00,0x00,0x00,0x00]` — exactly
   `set_configuration_packet(1)`. This triggers ChibiOS's
   `USB_EVENT_CONFIGURED` callback, which calls `usbInitEndpointI` for the
   real firmware endpoints — confirmed by the resulting register writes:
   - `oe[2].DOEPCTL = 0x1008_8040` and `ie[2].DIEPCTL = 0x1088_8040` —
     `EPTYP_BULK | USBAEP | MPSIZ(64)`, exactly `cdcDataEpConfig`
     (`USB_EP_MODE_TYPE_BULK`, 0x0040 in both directions).
   - `ie[3].DIEPCTL = 0x10cc_8010` — `EPTYP_INTR | USBAEP | MPSIZ(16)`,
     exactly `cdcInterruptEpConfig` (`USB_EP_MODE_TYPE_INTR`, 0x0010,
     IN-only).
   - `oe[2].DOEPCTL` is then re-armed with `EPENA` set
     (`0x9408_8040`) — the real bulk OUT endpoint is now actively
     listening, matching this project's `bulk_out_endpoint = Some(2)`.
4. **`SET_LINE_CODING`**: firmware reads back
   `[0x21,0x20,0x00,0x00, 0x00,0x00,0x07,0x00]` — exactly
   `set_line_coding_packet()`'s SETUP header (class request, interface
   recipient, `bRequest=0x20`, `wLength=7`). `DOEPINT.STUP` fires exactly
   when the `SETUP_COMP` entry is popped, confirming the interrupt-timing
   fix holds for this stage too.

The capture ended here (60 real seconds elapsed) before the SET_LINE_CODING
7-byte data stage (`OUT_DATA`/`OUT_COMP`, still queued in `rx_status` at
that point) and `SET_CONTROL_LINE_STATE` were processed — not a functional
problem, just the wall-clock window closing.

## A fourth bug, found by attempting a real TunerStudio byte exchange: firmware halts

Attempting the simplest real protocol exchange (send a single unframed `'Q'`
byte — rusEFI's plain "hello"/query command, no length or CRC framing
needed — and wait for an ASCII response; command bytes and framing
confirmed from `firmware/console/binary/tunerstudio.cpp` and
`firmware/integration/ts_protocol.txt`) reproducibly halted real firmware
instead of responding, in every capture. `pc` got stuck alternating between
two adjacent addresses (`nop; b self`) after `cpsid i` (interrupts disabled)
— a classic ChibiOS `chSysHalt` panic, not a stall.

Root-causing this precisely required *matching* debug symbols: the
`rusefi.bin` this project runs was built from a source snapshot ~12 days
older than this session's `epicefi_fw` checkout (confirmed by the differing
embedded `TS_SIGNATURE` build-date strings), so `addr2line` against the
current checkout's `epicefi.elf` produced nonsensical, scattered function
names for `rusefi.bin`'s addresses. The fix: build `epicefi.elf`/`epicefi.bin`
fresh from the current checkout (already done earlier this session) and
swap it in for verification — its vector table (initial SP `0x2002_1000`,
reset vector `0x0020_03d5`) matches `rusefi.bin`'s exactly, so it's a valid
drop-in replacement for this board config. With matching symbols,
`addr2line` resolved the exact call stack:

```
otg_epout_handler.constprop.0   hal_usb_lld.c:458   (STUP dispatch)
_usb_ep0setup                   hal_usb.c:863
hybridRequestHook               usbcfg.cpp:455       (-> sduRequestsHook)
_usb_ep0setup                   hal_usb.c:943         (OUT-phase: usbStartReceiveI)
usbStartReceiveI                hal_usb.c:471
chDbgCheckClassI                chdebug.c:251         (passes)
chSysHalt                       chsys.c:209           (<- osalDbgAssert fails)
__disable_irq                   cmsis_gcc.h:142        (cpsid i)
strlncpy                        efistring.cpp:7        (copying the panic reason)
```

The failing assertion is `hal_usb.c:476`:
`osalDbgAssert(!usbGetReceiveStatusI(usbp, ep), "already receiving");`

**Why it fails**: a real USB host acknowledges a `DEV2HOST` (IN-direction)
control transfer's data stage with a **zero-length OUT status packet**
before sending the next SETUP. `_usb_ep0in`'s completion handler
(`USB_EP0_IN_WAITING_TX0` case) arms EP0 to receive exactly this via
`usbStartReceiveI(usbp, 0, NULL, 0)`, which sets `usbp->receiving`'s bit for
that endpoint. This project's virtual host previously skipped that status
stage entirely and queued the next SETUP directly after `GET_DESCRIPTOR`'s
IN transfer completed — so the flag stays set forever. It's harmless until
the first *later* stage that itself calls `usbStartReceiveI`:
`SET_LINE_CODING` is the first (and, in this 5-stage sequence, only)
`HOST2DEV` stage with a real data phase, and its call hits the still-set
flag and fails the assertion — which is why the halt always happened at
exactly this point, never earlier.

Of the 5 stages this project drives, only `GET_DESCRIPTOR` is `DEV2HOST` —
`SET_ADDRESS`/`SET_CONFIGURATION`/`SET_CONTROL_LINE_STATE` have no data
stage at all (their status ack is an IN-direction zero-length transmit,
already handled correctly by this project's existing zero-length
`complete_in_transfer` path), and `SET_LINE_CODING` is `HOST2DEV` with data,
not `DEV2HOST`. So `GET_DESCRIPTOR` needed a one-off fix, not a general
per-stage mechanism.

**Fix** (`src/peripherals/otg_fs.rs`): added
`virtual_host_control_in_status_ack`, which queues a zero-length
`OUT_DATA`/`OUT_COMP` pair (clearing `usbp->receiving` via the existing
`GRXSTSP`-pop-driven `DOEPINT.XFRC` mechanism, no new plumbing needed), and
called it from `advance_virtual_host`'s `AwaitingDeviceDescriptor` case
before queuing the `SET_ADDRESS` SETUP packet.

**Verified**: re-ran the exact same `'Q'`-byte exchange attempt twice more
against the freshly built `epicefi.bin` (matching symbols) after the fix.
Both captures ran for 90+ seconds with the connection held open and showed
**zero** further `chSysHalt` occurrences (the only `cpsid i` in either
capture was the one already present during normal early boot) — a clean
before/after comparison against the identical binary, not just "it didn't
crash this specific run." Both captures also confirmed the `'Q'` byte
itself reaching firmware correctly: `GRXSTSP` popped `OUT_DATA`(bcnt=1,
ep=2) then `OUT_COMP`(ep=2) — exactly this project's encoding for a 1-byte
bulk OUT delivery on the real CDC data endpoint — and firmware's
`oe[2].DOEPINT` read back `0x01` (`XFRC`) and cleared it, meaning
`otg_epout_handler` genuinely processed the byte as a completed OUT
transfer.

**Not yet observed**: an actual ASCII response byte back over the TCP
socket. Both post-fix captures show firmware going on to activate endpoint
1 (`USB_MSD_DATA_EP = 0x01` per
`ChibiOS-Contrib/os/hal/include/hal_usb_msd.h:36` — rusEFI's SD-card-as-USB-
mass-storage feature, unrelated to CDC) and then simply not touching
OTG-FS registers again for 70M+ further instructions even with the TCP
connection held open the whole time. This looks like real firmware being
busy with its own thread scheduling (most likely the MSD mount) rather than
a further bug in this project's model — every register-level interaction
observed has matched expectations exactly — but getting a visible
end-to-end response within a practical `-vvvv` capture window (which
trades off badly against real wall-clock time; see "Live capture attempts"
below) remains unconfirmed.

### Live capture attempts

- Connecting a few seconds after boot and holding 5-30s: reached the first
  `GET_DESCRIPTOR` SETUP/response exchange (documented above) but no
  further — bottlenecked by `-vvvv`'s I/O throughput as the log grew past
  tens of millions of lines.
- Connecting immediately (before firmware's own ~57M-instruction boot init)
  and holding for 280s real time (82M+ instructions logged): the OTG-FS
  reset-response burst that reliably appears in every other capture (GRSTCTL
  TXFFLSH, DAINTMSK, GRXFSIZ, etc. — see above) never happened at all, even
  though `virtual_host_reset()` fired within the first few million
  instructions. This suggests connecting before firmware has enabled/unmasked
  the OTG-FS interrupt in the NVIC can cause the pending interrupt to never
  actually be serviced — a real behavior worth investigating, but out of
  scope for this task.
- One `--max-instructions`-bounded, `-vvv` run finished in under a second of
  wall-clock time without a client ever staying connected long enough to
  matter (the connect script didn't hold the socket open), confirming
  nothing about firmware behavior.

## A fifth bug, found by source-reading rather than live capture: no SOF interrupt ever fires

Chasing the "no visible TunerStudio response" symptom further (the fourth
bug's fix let the `'Q'` byte reach firmware cleanly, but no response byte was
ever observed coming back), a live `-vvvv` capture confirmed — via
matching-symbol `addr2line` against the same freshly-built `epicefi.elf` —
that the response *is* fully computed and queued:

- `TunerstudioThread::ThreadTask` wakes (`0x0023576c`).
- `handleQueryCommand` (`0x0026c3cc`) and `TsChannelBase::sendResponse`
  (`0x0028382c`) each execute exactly once (confirmed via exact `pc=` grep
  counts against the full capture log).
- A disassembly trace from `sendResponse`'s entry showed the ~46-byte
  signature string being copied into ChibiOS's serial-over-USB output
  buffers queue (`obqWriteTimeout`, via `UsbChannel::write` in
  `usb_console.cpp` -> `chnWriteTimeout`).
- Immediately after, the CPU hits an `svc #0` context switch that returns to
  the **idle thread** (`wfi` loop) — no further OTG-FS register write ever
  occurs. The byte sits in the queue and nothing transmits it.

Reading ChibiOS source (`hal_buffers.c`, `hal_serial_usb.c`,
`hw_layer/ports/stm32/serial_over_usb/usbcfg.cpp`) found the exact
mechanism, and a static check against the full capture log confirmed it
directly: `obqWriteTimeout`/`obqPutTimeout` only invoke the output queue's
`notify` callback (`obnotify` in `hal_serial_usb.c`, wired up as
`SDU1.obqueue.notify`) from `obqPostFullBufferS` — which only runs when a
buffer is either **completely filled** (64 bytes, `SERIAL_USB_BUFFERS_TX_SIZE`)
or explicitly force-posted. `TsChannelBase::flush()` (called right after
`write()` in `sendResponse`) is a **no-op by default**
(`tunerstudio_io.h:38`), and `usb_console.cpp`'s `UsbChannel` doesn't
override it — so a short response (well under 64 bytes) never triggers
`obnotify` through the write/flush path at all. Grepping the full capture
log for `obnotify`'s address (`0x0020aab8`, found via `nm`) confirmed **zero**
occurrences, versus one for `handleQueryCommand` in the same log — `obnotify`
is never called, period.

The actual flush mechanism is `sduSOFHookI` (`hal_serial_usb.c:407`), the
USB Start-of-Frame interrupt handler: on real hardware a host emits a SOF
every 1ms *regardless of data activity*, and ChibiOS wires
`usbcfg.cpp`'s `sof_handler` (`USBConfig.sof_cb`) to call it on every one.
Because `sof_cb` is non-`NULL`, `hal_usb_lld.c`'s ISR (line ~629) never
disables `GINTMSK.SOFM` — real firmware expects a continuous SOF heartbeat.
`sduSOFHookI` checks `obqTryFlushI` and force-posts any partially-filled
buffer, which is the *only* way a short response ever gets transmitted.

This project's virtual USB host never modeled SOF at all — it only reacts to
explicit control/bulk transfers it initiates, never generating the
bus's continuous background heartbeat. So enumeration (all explicit
SETUP/IN/OUT sequences) and bulk-OUT delivery (also explicit, via `poll()`'s
bridge-forwarding) both worked correctly, but nothing ever gave firmware a
reason to flush a short bulk-IN response.

**Fix** (`src/peripherals/otg_fs.rs`): added `GINTSTS_SOF` (bit 3) and raise
it on every `poll()` tick while a virtual host is attached — `poll()` already
runs roughly every 100,000 instructions
(`framebuffers::sdl_engine::PUMP_EVENT_INST_INTERVAL`, "~1-10ms" per its own
comment), which is already the right order of magnitude for a SOF heartbeat,
so no new timing mechanism was needed. This is a plain status bit already
handled generically by the existing W1C `GINTSTS` write path and
`effective_gintsts()`/`interrupt_pending()` masking logic — no other
plumbing required.

Found entirely by source-reading, not live capture (this session's
"should be instant" lesson: long real-time waits don't scale, but grepping a
capture log for one specific address does).

**Verified end-to-end**: with the fix applied, a fresh `-vvv` capture against
the same `epicefi.bin`, connecting well after boot (`clk` > 240M, firmware
already idling normally), sending a raw `'Q'` byte over the TCP bridge got
back the full 46-byte signature response
(`epicEFI Tera.2026.07.11.proteus_f7.1877407474\0`) in **0.21 seconds** real
time — not minutes. This is the first fully successful TunerStudio protocol
round-trip against this project's virtual USB host, closing out the
investigation this document has been tracking since the fourth bug.
