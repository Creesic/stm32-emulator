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
