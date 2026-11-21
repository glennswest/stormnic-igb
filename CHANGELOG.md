# Changelog

## [Unreleased]

### 2026-11-20
- **fix:** The PHY semaphore stayed held when a register access failed
  (#10, finding bf153eab9dc0): `with_phy` and `take_swsm` returned on a
  SWSM or SW_FW_SYNC read/write error before giving SWSM back, which could
  lock firmware or a BMC out of the shared PHY until reset. Every path
  after SMBI is taken now gives SWSM back before returning the error; a
  failed write or read-back of the PHY bit clears the bit; the release
  clears the bit even when SWSM cannot be taken again, and the PHY work's
  own error is returned over the release's. Host tests in `test/igb.rs`
  with a simulator that fails one register access.
- **fix:** SNP transmit completion (#9). GetStatus handed a token back as
  soon as one was queued, before the controller had written DD into its
  descriptor (finding b78ceb3a5cda); each token now carries its descriptor's
  sequence number and comes back only once the descriptor is retired, and
  TRANSMIT_INTERRUPT is reported when that completion is first seen. Reset
  and Initialize rewrote the transmit ring but kept the queue and the
  interrupt flag (finding a2e462520655): frames a ring reset discards are
  now marked aborted, handed back by GetStatus (the caller's buffer was
  never used for DMA) with no transmit interrupt pending; Stop and
  ExitBootServices drop the queue. A relink error in Initialize or Reset
  left RX, TX and DMA running with the interface reported failed (finding
  e6874a9b956a): the NIC is quiesced and the interface is Started. Host
  tests in `test/snp.rs`; the simulator can hold transmit descriptors.
- **fix:** A NIC that would not quiesce (a failed MAC reset or quiesce in
  Start, an SNP that could not be uninstalled, a binding Stop whose quiesce
  failed) had bus mastering cleared and then the original PCI attributes put
  back, which turned bus mastering on again when the firmware had it on: the
  abandoned NIC could DMA into the kept region (#7, finding ec68cd8a67bf).
  Those paths now use `decode::abandon`: the original attributes without
  BUS_MASTER, the command register's MSE as before, BME verified clear.
  Host tests for `decode` (`test/decode.rs`). #8 is the same finding filed
  again; re-checked at 48a12c4 (no quiesce-failure path reaches `restore`,
  host tests pass) and closed as fixed by ec7a5b6.

### 2026-11-14
- **docs:** README status brought to v0.2.0 (stormbootx pin, open #6), a
  garbled console paragraph repaired; CLAUDE.md work plan notes #6.

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
