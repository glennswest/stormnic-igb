# Changelog

## [Unreleased]

## [v0.2.0] — 2026-11-13

### Added
- The stormnic contract (#5): the driver owns its port's bring-up
  (PHY powered and autonegotiating, link, the IEEE standard FEC for the
  media and speed: none for every igb media/speed, `igb::standard_fec`) and
  prints one line per port: driver, PCI address, PCI ID, part, LAN function,
  MAC, media, speed, FEC, link. A port without link is an `error:` line.
  The old per-NIC line's flow-control/SNP details moved to the trace.

## [v0.1.0] — 2026-11-07

### Fixed
- After a MAC reset, autonegotiation is restarted when STATUS.LU stays down
  while the PHY's BMSR has link (QEMU's 82576 raises LU only at the end of an
  autonegotiation; #3).
- Receive uses the advanced one-buffer descriptor format (QEMU's 82576
  writes no legacy receive descriptors; #3).
- The PHY-busy diagnostic no longer reads SWSM (a read takes the semaphore).
- An unused constant failed the build under `warnings = "deny"` (#2).

### 2026-11-07
- **feat:** the igb UEFI SNP driver (#1): EFI driver binding for 36 igb
  physical-function IDs (82575, 82576, 82580, I350, I354, I210, I211,
  checked against pci.ids 2026.11.06); MAC reset with bus-master drain, the
  NVM's station address, flow control off (TFCE/RFCE clear, XON/XOFF zero,
  SRRCTL.DROP_EN), queue 0 with legacy descriptors, the copper PHY powered
  up and autonegotiating under the SW/FW semaphore (never reset), a DMA check
  at Start, the NIC quiesced in every state but SNP Initialized and at
  ExitBootServices; one console line per NIC, the trace behind
  `StormnicVerbose`.
- **test:** simulated-controller host tests (`scripts/test-host.sh`); the
  QEMU `igb` + OVMF end-to-end check (`scripts/test-ovmf.sh`, check app in
  `check/`); x86_64 + aarch64 build and PE subsystem check
  (`scripts/check-driver.sh`).
- **docs:** README, docs/hardware.md, NOTICE (sources), CLAUDE.md work plan.

### 2026-10-07
- **chore:** repository created (owner: low priority, paused)
