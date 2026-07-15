# anatomy.md

> Auto-maintained by OpenWolf. Last scanned: 2026-07-15T16:23:19.038Z
> Files: 123 tracked | Anatomy hits: 0 | Misses: 0

## ../../../AppData/Local/Temp/claude/C--Users-Tera-Documents-GitHub-stm32-emulator/2ddd4150-2756-4300-8896-52f47e9b74d4/scratchpad/

- `feed_harness.py` (~98 tok)

## ../../../AppData/Local/Temp/claude/C--Users-Tera-Documents-GitHub-stm32-emulator/432d2e68-e981-4da6-a522-9b21d7862bdc/scratchpad/

- `abandon_modes.py` — Exercise three client-abandonment modes against the USB CDC TCP bridge (~578 tok)
- `config.yaml` (~192 tok)
- `ecu_io_client.py` — send (~124 tok)
- `elfcompare.py` (~383 tok)
- `enum_probe.py` — Minimal client: connect, send TS signature request, hold open ~9s so the (~263 tok)
- `exact_live_config.yaml` (~388 tok)
- `find_callers.sh` (~179 tok)
- `find_stacks.sh` (~184 tok)
- `findobnotify.sh` (~81 tok)
- `findsyms.sh` (~93 tok)
- `gui_equivalent_config.yaml` (~372 tok)
- `probe_ts.py` (~146 tok)
- `probe_ts2.py` (~200 tok)
- `replay_cmd.py` (~298 tok)
- `replay_exact_r.py` — send_and_read (~458 tok)
- `replay_f.py` (~200 tok)
- `replay_framed.py` (~224 tok)
- `replay_full.py` — framed, send_and_read (~320 tok)
- `replay_leak_check.py` — framed (~358 tok)
- `replay_seq.py` — framed, send_and_read (~275 tok)
- `resolve_can.sh` (~65 tok)
- `resolve_idle_seq.sh` (~90 tok)
- `resolve_tim5.sh` (~103 tok)
- `resolve_vectors.sh` (~104 tok)
- `resolve.sh` (~82 tok)
- `resolve2.sh` (~83 tok)
- `resolve3.sh` (~76 tok)
- `symlookup.py` — find (~289 tok)
- `ts_proxy.py` — log, relay, handle_client (~671 tok)
- `ts_query.py` (~169 tok)

## ./

- `.gitignore` — Git ignore rules (~3 tok)
- `AGENTS.md` — AGENTS.md (~2800 tok)
- `asm.py` — pip3 install keystone-engine (~141 tok)
- `build.rs` (~58 tok)
- `Cargo.toml` — Rust package manifest (~190 tok)
- `CLAUDE.md` — Build/test/architecture guide incl. native launcher, Proteus F7, virtual USB (~2200 tok)
- `LICENSE` — Project license (~9553 tok)
- `README.md` — Project documentation (~2768 tok)

## .claude/

- `settings.json` (~441 tok)

## .claude/rules/

- `openwolf.md` (~313 tok)

## .superpowers/sdd/

- `final-review-fixes-report.md` — Final Review Fixes Report — Proteus F7 ECU I/O (6 Minor Findings) (~2590 tok)
- `task-1-report.md` — Task 1 Report: Move Proteus F7 board data into launcher boards module (~1095 tok)
- `task-2-report.md` — Task 2 Report: Grow launcher tables to full harness map (TDD) (~1183 tok)
- `task-3-report.md` — Task 3 Report: Full Proteus F7 Harness in config.yaml (~945 tok)
- `task-4-report.md` — Task 4 Report: Update `docs/proteus-f7-ecu-io.md` to the new harness signal naming (~1802 tok)
- `task-5-report.md` — Task 5 Report: Live end-to-end verification of the full Proteus F7 harness I/O routing (~1779 tok)

## .wolf/

- `anatomy.md` — Repository file map and navigation guidance (~650 tok)
- `buglog.json` — Structured record of encountered errors and fixes (~50 tok)
- `cerebrum.md` — Cross-session project conventions and learnings (~150 tok)
- `memory.md` — Chronological OpenWolf action log (~100 tok)
- `OPENWOLF.md` — OpenWolf operating protocol (~1300 tok)

## docs/

- `proteus-f7-ecu-io.md` — Proteus F7 ECU I/O (~1725 tok)
- `proteus-f7-usb.md` — Proteus F7 Virtual USB CDC (~1104 tok)

## docs/superpowers/plans/

- `2026-07-10-proteus-f7-bringup.md` — Task-by-task plan for reproducible Proteus F767 boot verification (~2400 tok)
- `2026-07-10-proteus-f7-virtual-usb.md` — Proteus F7 Virtual USB Implementation Plan (~12781 tok)
- `2026-07-11-proteus-f7-ecu-io.md` — Proteus F7 ECU I/O Implementation Plan (~17036 tok)
- `2026-07-13-launcher-process-status.md` — Launcher Process Status Indicator Implementation Plan (~5917 tok)
- `2026-07-15-proteus-harness-io.md` — Proteus F7 Full Harness I/O Routing Implementation Plan (~6500 tok)
- `2026-07-15-proteus-harness-io.md` — Proteus F7 Full Harness I/O Routing Implementation Plan (~6436 tok)

## docs/superpowers/specs/

- `2026-07-10-native-launcher-design.md` — Approved static-profile native launcher design (~700 tok)
- `2026-07-10-proteus-f7-bringup-design.md` — Approved staged boot and hardware-modeling design (~650 tok)
- `2026-07-11-proteus-f7-ecu-io-design.md` — Proteus F7 ECU I/O Design (~2237 tok)
- `2026-07-13-launcher-process-status-design.md` — Launcher Process Status Indicator Design (~1327 tok)
- `2026-07-15-proteus-harness-io-design.md` — Proteus F7 Full Harness I/O Routing Design (~1600 tok)
- `2026-07-15-proteus-harness-io-design.md` — Proteus F7 Full Harness I/O Routing Design (~1310 tok)

## monox/

- `config.yaml` (~601 tok)

## proteus_f7/

- `boot-sequence-notes.md` — Proteus F7 boot-sequence trace evidence (~3758 tok)
- `config.yaml` (~1226 tok)
- `epicefi_symbols.yaml` — TEMPORARY diagnostic config: identical to config.yaml but loads the (~321 tok)
- `README.md` — Local asset setup and bounded trace commands (~90 tok)
- `setup.ps1` — Copies firmware and extracts the exact F767 SVD from ST's archive (~190 tok)
- `usb_trace_notes.md` — Proteus F7 OTG-FS trace evidence (~6445 tok)
- `verify_boot.ps1` (~724 tok)
- `verify_fpu.ps1` — Bounded post-VDIV Cortex-M7 smoke test for local Proteus F767 assets (~430 tok)

## saturn/

- `config.yaml` (~510 tok)

## src/

- `emulator.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~3605 tok)

## src/bin/

- `config.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~221 tok)
- `emulator.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~2717 tok)
- `main.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1211 tok)
- `stm32-launcher.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~6812 tok)
- `system.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1152 tok)
- `util.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~974 tok)

## src/ext_devices/

- `display.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1865 tok)
- `ecu_io.rs` — Defensive cap on `recv_buffer`/`outgoing` growth against a stalled or malicious (~5786 tok)
- `lcd.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1257 tok)
- `mod.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1889 tok)
- `spi_flash.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1135 tok)
- `touchscreen.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1527 tok)
- `usart_probe.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~328 tok)
- `usb_cdc_tcp.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~4480 tok)

## src/framebuffers/

- `image.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~624 tok)
- `mod.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~611 tok)
- `sdl_engine.rs` — How often should we call pump_events() in terms of number of instructions emulated (~668 tok)
- `sdl.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1158 tok)

## src/launcher/

- `mod.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~2425 tok)
- `process.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1803 tok)
- `ui_state.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~175 tok)
- `workspace.rs` — [derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)] (~918 tok)

## src/launcher/boards/

- `mod.rs` — Per-board launcher profile data. One module per supported board: (~83 tok)
- `proteus_f7.rs` — Proteus F7 (STM32F767) launcher profile: memory map, boot patches, (~1750 tok)

## src/peripherals/

- `adc.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~2512 tok)
- `dma.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~4540 tok)
- `dwt.rs` — ARM CoreSight DWT unit. Firmware uses DWT->CYCCNT for microsecond-precision (~462 tok)
- `exti.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1930 tok)
- `flash.rs` — minimal FLASH ACR latency model for F767 startup (~250 tok)
- `fsmc.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1537 tok)
- `gpio.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~2184 tok)
- `i2c.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~386 tok)
- `mod.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~5164 tok)
- `nvic.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~7634 tok)
- `otg_fs.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~12464 tok)
- `pwr.rs` — Minimal PWR CR1/CSR1 voltage-scaling readiness model (~250 tok)
- `rcc.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~256 tok)
- `scb.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1104 tok)
- `spi.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~906 tok)
- `sw_spi.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~1032 tok)
- `systick.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~512 tok)
- `tim11.rs` — TIM11 capture-ready status model used by Proteus F7 startup (~300 tok)
- `tim5.rs` — TIM5's global interrupt, fixed at this NVIC position across the whole (~2414 tok)
- `usart.rs` — SPDX-License-Identifier: GPL-3.0-or-later (~615 tok)

## tests/

- `launcher_profile.rs` — Full Proteus harness pin table (name, pin, direction), copied from (~3335 tok)
- `launcher_state.rs` — [test] (~282 tok)
