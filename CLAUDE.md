# CLAUDE.md — stormnic-igb

A `no_std` UEFI driver giving firmware an `EFI_SIMPLE_NETWORK_PROTOCOL` for
the **Intel igb** gigabit family: 82575, 82576, 82580, I350, I354, I210 and
I211 (`src/ids.rs`, checked against pci.ids). Loaded by stormbootx from
`\stormboot\drivers` like its siblings stormnic-ixgbe, stormnic-mlx4,
stormnic-virtio and stormnic-realtek. Read README.md first.

Read the cross-project rules in `../CLAUDE.md` first. In particular, **build
with `sc-build` after pushing, never on this VM and never as root**, and
scratch files go in `tmp/`.

## Rules for this crate

- **Sources:** register offsets, bit fields and descriptor formats from
  Intel's public datasheets (82576, I350 and I210 Ethernet Controller
  datasheets; the legacy descriptor formats are the 8254x ones), and the UEFI
  specification for the binding and SNP. iPXE's `intel.c` and Linux's igb
  (both GPL) are read for behaviour only, **never translated**: the crate is
  MIT. docs/hardware.md describes the hardware in its own words.
- It is a *driver*: the binary is an EFI boot-service driver (subsystem 11)
  installing `EFI_DRIVER_BINDING_PROTOCOL`. Supported declines a function
  another driver already holds BY_DRIVER: the platform's own driver (Intel's
  UNDI) wins unless a loader disconnects it first (stormbootx
  `prefer_media_drivers = igb`, the check app).
- **Flow control is off** (as stormnic-realtek#2): CTRL.TFCE/RFCE clear and
  FCRTL/FCRTH zero, and SRRCTL.DROP_EN set, so a receive ring nobody drains
  drops frames instead of sending pause frames to the switch.
- **The NIC is quiet whenever nobody drives it**: RX/TX and both queues
  disabled, interrupts masked, bus-master requests drained and the MAC reset
  at every Start failure, SNP Shutdown/Stop, binding Stop and
  ExitBootServices. Only SNP Initialize enables RX and TX.
- The PHY is never reset (the link the firmware or power-on brought up
  stays); Start only makes sure autonegotiation is on and the PHY powered,
  and only under the software/firmware PHY semaphore (a BMC may share the
  port on I350/I210 boards).
- Same layout as stormnic-realtek: the hardware core (`src/igb.rs`,
  `src/ring.rs`, `src/snp_core.rs`) is UEFI-independent and tested on the
  host against a simulated controller (`test/`); `src/binding.rs`,
  `src/snp.rs`, `src/pci_io.rs` are the UEFI glue.
- One line per NIC on the console by default, the trace behind
  `StormnicVerbose` / `--features verbose` (stormnic-ixgbe#22).

## Build

```bash
sc-build scripts/check-driver.sh     # build + clippy -D warnings + PE32+ subsystem 11
sc-build scripts/test-host.sh        # simulated-controller tests
sc-build scripts/test-ovmf.sh        # QEMU igb (82576) + OVMF: the check app leases, pings, echoes
```

## Test

- Host: `test/*.rs`, compiled with plain `rustc --test` (scripts/test-host.sh).
- QEMU/OVMF (`scripts/test-ovmf.sh`): QEMU's `igb` device (QEMU 8.0+) is an
  82576 (8086:10c9). The check app (`check/`) disconnects whatever firmware
  driver holds it, loads `\stormboot\drivers\stormnic-igb.efi`, connects it
  and runs smoltcp over its SNP: DHCP, ping the gateway, UDP echo, then
  prints `STORMNIC-IGB CHECK PASS`.
- No I350/I210/82580 in the lab (stormcentral asset list, 2026-11-07); those
  are verified in simulation only until a machine with one boots it.

## Version

`Cargo.toml` → `package.version` (and `check/Cargo.toml`). Current: `v0.2.0`.

## Work plan

- [x] #1 UEFI SNP driver: bind, link, transmit/receive (closed 2026-11-07;
      the pin is stormbootx#69's, which waited on #1)
  - [x] Scaffold: workspace (driver + `check/`), build.rs subsystem 11,
        glue from stormnic-realtek, PCI IDs from pci.ids 2026.11.06 (0dc68b6)
  - [x] Hardware core (reset, MAC address, PHY check, rings), SNP core
  - [x] Host tests: `sc-build scripts/test-host.sh` at 1d5b3d2, 46 pass
        (igb 29, snp 14, trace 3)
  - [x] `sc-build scripts/check-driver.sh` at 1d5b3d2: x86_64 59,904 B
        (sha256 408c7e66…786b0) and aarch64 56,320 B, PE32+ subsystem 11,
        clippy -D warnings clean (#2, a dead-code error, fixed in 31e6bdf)
  - [x] QEMU/OVMF igb (`sc-build scripts/test-ovmf.sh`) at 1d5b3d2, QEMU
        10.2.2: both boots PASS (lease 10.0.2.15, ping 5/5, UDP echo 1200 B).
        Two QEMU-model findings fixed on the way (#3): STATUS.LU comes back
        only after an autonegotiation (98ea20f `relink`), and receive
        descriptors are written in the advanced format only (1d5b3d2)
  - [x] stormbootx: pin 80c9ea5 (v0.1.0) in build-nic-drivers.sh, asked
        on stormbootx#69 (its own work, released by closing #1)
  - [ ] Later, not blocking: a real I350/I210/82576 (none in the lab,
        2026-11-07); the relink path was only seen on QEMU
- [x] #5 The stormnic contract: the driver brings its port up itself
      (speed, link, the IEEE standard FEC for media/speed) and reports one
      line per port: driver, PCI address, MAC, media, speed, FEC, link
  - [x] `igb::standard_fec` (none for every igb media/speed: 802.3 defines
        no FEC for 10/100/1000BASE-T, 1000BASE-X/KX, SGMII, 2500BASE-KX)
        and `igb::PortLine`, host-tested
  - [x] binding prints the PortLine; link down is an `error:` line (b065f80)
  - [x] test-ovmf checks the line (`FEC none`, `link UP`); v0.2.0.
        At b065f80: host tests, check-driver (x86_64 60,928 B, aarch64
        56,832 B, subsystem 11, clippy clean) and both QEMU igb + OVMF boots
        pass with `… 82576 port 0: MAC 52:54:00:12:34:56, media copper,
        speed 1000 Mb/s full duplex, FEC none, link UP`
  - [ ] Later: stormbootx's pin (build-nic-drivers.sh) moves to v0.2.0 —
        its own work; a real igb card when one is in the lab
