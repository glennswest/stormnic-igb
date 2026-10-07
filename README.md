# stormnic-igb

A Rust `no_std` UEFI Simple Network Protocol (SNP) driver for **Intel igb (i350, i210/i211, 82576/82580)** (8086:1521–1524, 1533–1539, 157B/157C, 10C9, 150E), loaded by
[stormbootx](https://github.com/glennswest/stormbootx) so it can claim and boot images over these NICs without iPXE.
A sibling of stormnic-ixgbe, stormnic-mlx4 and stormnic-virtio, built and pinned the same way (stormbootx `build-nic-drivers.sh`).

Status: not started (P3, paused; owner 2026-10-07).

## Licence

MIT (see LICENSE). NOTICE acknowledges the sources the hardware facts were learned from; no code was copied.
