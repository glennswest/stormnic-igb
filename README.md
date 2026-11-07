# stormnic-igb

A Rust `no_std` UEFI driver that gives firmware an
`EFI_SIMPLE_NETWORK_PROTOCOL` (SNP) for the **Intel igb** gigabit family:
82575, 82576, 82580, I350, I354, I210 and I211, built for x86_64 and
aarch64. [stormbootx](https://github.com/glennswest/stormbootx) loads it
from `\stormboot\drivers` and runs its own TCP/IP (smoltcp) on it, as with
its siblings stormnic-ixgbe, stormnic-mlx4, stormnic-virtio and
stormnic-realtek.

Hardware facts from Intel's public datasheets (see NOTICE and
docs/hardware.md), the binding and SNP from the UEFI specification. MIT.

Status: v0.1.0. Verified against a simulated controller and end to end
under OVMF with QEMU's `igb` device (an emulated 82576, `scripts/test-ovmf.sh`).
No machine in the lab has an igb part yet, so it has not run on real
silicon. Not yet pinned in stormbootx's `build-nic-drivers.sh`.

## What it binds

Every igb physical function in `src/ids.rs` (36 IDs, checked against
pci.ids 2026.11.06):

| MAC | PCI IDs (8086:) |
|---|---|
| 82575 | 10a7, 10a9, 10d6, 10e2 |
| 82576 | 10c9, 10e6, 10e7, 10e8, 150a, 150d, 1518, 1526 |
| 82580 | 1509, 150e, 150f, 1510, 1511, 1516, 1527 |
| I350 | 1521, 1522, 1523, 1524 |
| I354 | 1f40, 1f41, 1f45 |
| I210 | 1533, 1534, 1535, 1536, 1537, 1538, 157b, 157c, 15f6 |
| I211 | 1539 |

Not bound: the virtual functions (10ca, 1520, 152f), the blank-NVM I210
(1531), and the e1000e (82574, PCH I217–I219: stormnic-e1000e) and igc
(I225/I226) parts. Copper ports get their PHY checked; fiber, SerDes,
backplane and SGMII ports are driven with the link the NVM set up.

The driver declines a function another driver already holds `BY_DRIVER`, so
the platform's own driver (Intel's UNDI in the board firmware) wins.
stormbootx takes the NIC from it with `prefer_media_drivers = igb`.

## How it works

- **Start**: PciIo `BY_DRIVER | EXCLUSIVE`; memory decode and bus mastering
  (`src/decode.rs`); BAR 0; STATUS, the link mode and the station address
  (RAL0/RAH0). The MAC is reset (bus-master requests drained first) and the
  address the NVM reload puts back is used. One DMA region (PciIo
  `AllocateBuffer` + `Map` common buffer, 33 pages) holds both rings and
  their buffers. The NIC is brought up once with flow control off; on a
  copper port the internal PHY is powered up and autonegotiation turned on
  if a previous driver left it otherwise (under the software/firmware
  semaphore, never a PHY reset); with the link up a broadcast frame checks
  DMA. Then it is quiesced again and the SNP goes on a child handle with a
  MAC device-path node.
- **SNP**: only Initialize turns RX/TX on; Shutdown, Stop, binding Stop and
  ExitBootServices quiesce it (receive and transmit off, queues disabled,
  MAC reset). Polled, one queue (advanced one-buffer receive descriptors,
  legacy transmit descriptors), 32 + 32 buffers of
  2 KB, frames copied in and out. The controller accepts its address,
  broadcast and all multicast; the SNP receive filters are applied in
  software (promiscuous in RCTL).
- **Flow control is off**: CTRL.TFCE/RFCE clear, XON/XOFF thresholds zero,
  and a full receive ring drops (SRRCTL.DROP_EN) instead of sending pause
  frames to the switch.
- **Console**: one line per NIC, e.g.
  `stormnic-igb 0.1.0: 0000:00:02.0 8086:10c9 82576 (82576, copper): MAC 52:54:00:12:34:56, link up 1000 Mb/s full duplex, flow control off, SNP installed`,
  plus warnings. The whole bring-up trace is printed when the volatile EFI
  variable `StormnicVerbose` (GUID `ce1479a2-eab9-4176-b0ad-c909ea5b8e0b`,
  shared by the stormnic drivers) is set, or with `--features verbose`; a
  failure replays the last 16 trace lines first.

## Build and test

Never on the session VM: with `sc-build` after pushing.

```bash
sc-build scripts/check-driver.sh   # x86_64 + aarch64, clippy -D warnings, PE32+ subsystem 11
sc-build scripts/test-host.sh      # the hardware core and SNP against a simulated controller
sc-build scripts/test-ovmf.sh      # QEMU igb + OVMF: DHCP, ping, UDP echo over the SNP
```

`scripts/test-ovmf.sh` builds a boot disk (`scripts/build-image.sh`) whose
`BOOTX64.EFI` is the check app (`check/`): it loads
`\stormboot\drivers\stormnic-igb.efi`, connects it to the igb function
(taking it from a firmware driver first if one holds it), opens the SNP
exclusively and runs smoltcp: a DHCP lease, five pings to the gateway and a
1200-byte UDP echo, then `STORMNIC-IGB CHECK PASS`. It boots twice: with the
quiet console and with `StormnicVerbose` set (`verbose=1` in
`\stormnic-check.conf`). Needs QEMU 8.0 or later for the `igb` device.

## Licence

MIT (see LICENSE). NOTICE acknowledges the sources the hardware facts were
learned from; no code was copied.
