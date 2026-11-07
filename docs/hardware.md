# The igb hardware, as this driver uses it

Written here from Intel's public datasheets (82576, I350, I210); see
NOTICE. Offsets are in memory BAR 0, all registers 32 bits wide.

## Family

One register model from the 82575 to the I210: the 82575/82576 (two or
four ports), the 82580 and I350 (up to four), the I354 (an SoC MAC on Atom
C2000), and the single-port I210/I211. Every port is its own PCI function;
STATUS bits 3:2 give its number. The VFs (82576 and I350 SR-IOV) are not
bound: they have no NVM or PHY of their own.

CTRL_EXT bits 23:22 (loaded from the NVM) choose the link: 00 the internal
copper PHY, 01 1000BASE-KX, 10 SGMII to an external PHY, 11 SerDes. Only
the copper PHY is reached here (MDIO address 1 through MDIC); the others
are left as the NVM set them and their link is read from STATUS.

## Registers used

| Offset | Name | Use here |
|---|---|---|
| 0x0000 | CTRL | GIO master disable (2), set link up (6), force speed/duplex (11, 12), software reset (26, self-clearing), RX/TX flow control (27, 28), VLAN mode (30), PHY reset (31) |
| 0x0008 | STATUS | full duplex (0), link up (1), function (3:2), speed (7:6: 10/100/1000), bus-master requests pending (19) |
| 0x0010 | EECD | NVM auto-read done (9) after a reset |
| 0x0018 | CTRL_EXT | link mode (23:22), driver loaded (28) |
| 0x0020 | MDIC | PHY access: data 15:0, register 20:16, PHY 25:21, op 27:26 (01 write, 10 read), ready 28, error 30 |
| 0x00c0 | ICR | interrupt causes, read to clear |
| 0x00d8 | IMC | interrupt mask clear (write ones) |
| 0x1528 | EIMC | extended (MSI-X) interrupt mask clear |
| 0x0100 | RCTL | receive enable (1), unicast promiscuous (3), multicast promiscuous (4), broadcast accept (15), strip CRC (26) |
| 0x0400 | TCTL | transmit enable (1), pad short packets (3), collision threshold (11:4), collision distance (21:12), retransmit on late collision (24) |
| 0x0170 | FCTTV | pause timer value |
| 0x2160, 0x2168 | FCRTL, FCRTH | XON / XOFF receive thresholds |
| 0x5200 | MTA[128] | multicast hash table |
| 0x5400, 0x5404 | RAL0, RAH0 | station address; RAH bit 31 "address valid". The NVM's address is loaded at every reset |
| 0x5818 | MRQC | multiple receive queues (RSS); 0 = queue 0 only |
| 0x5b50 | SWSM | semaphore: SMBI (0, set by the read that finds it clear), SWESMBI (1, sticks only when firmware does not have it) |
| 0x5b5c | SW_FW_SYNC | resource ownership: software bits 15:0, firmware bits 31:16 (the same resource 16 bits up). PHY of port 0/1/2/3: 0x02/0x04/0x20/0x40 |
| 0xc000… | queue 0 receive | RDBAL, RDBAH, RDLEN (+8, multiple of 128), SRRCTL (+0xc), RDH (+0x10), RDT (+0x18), RXDCTL (+0x28) |
| 0xe000… | queue 0 transmit | TDBAL, TDBAH, TDLEN, TDH (+0x10), TDT (+0x18), TXDCTL (+0x28) |

SRRCTL: packet buffer size in KB (6:0), descriptor type (27:25; 000 the
legacy format, 001 advanced with one buffer, the one used here), drop when
no descriptor is free (31). RXDCTL/TXDCTL:
prefetch, host and write-back thresholds (4:0, 12:8, 20:16) and the queue
enable (25), which reads back set once the queue runs and clear once it has
stopped.

## Sequences

**Quiesce** (every path that leaves the NIC): mask all interrupts, receive
off, both queues off (wait up to 10 ms for the enable bits to clear),
transmit off, then the reset below.

**Reset**: set CTRL.GIO master disable and wait (up to 8 ms) for
STATUS bit 19 to clear, so no DMA is cut off mid-transfer; then CTRL.RST.
It clears itself within milliseconds; the MAC, rings and filters go back to
their defaults and the NVM is reloaded (EECD bit 9), which puts the station
address back in RAL0/RAH0. The PHY is not reset, so its link survives; the
MAC sees it again once CTRL.SLU is set.

**Bring-up** (`mac_init`, then the driver's ring fill): CTRL with link up,
nothing forced, flow control off; FCRTL/FCRTH/FCTTV zero; RAL0/RAH0 with
the address and AV; MTA zero; MRQC zero; the receive queue's ring, length,
heads/tails at 0 and SRRCTL (2 KB, advanced one-buffer, drop enabled), then RXDCTL's
enable (write-back threshold 1) and wait for it; the same for transmit;
TCTL on; CTRL_EXT driver-loaded. Then the driver posts every receive
buffer, moves RDT to count − 1 and turns RCTL on (strip CRC, broadcast,
all multicast, unicast promiscuous on request).

**PHY** (copper only): take SWSM (SMBI then SWESMBI), check that neither
software nor firmware holds the port's PHY bit in SW_FW_SYNC, set it, give
SWSM back; do the MDIC accesses; give the bit back the same way. A BMC
sharing the port through NC-SI or SMBus uses the same semaphore. The only
PHY register touched is BMCR (0): if it has power-down (11) or isolate (10)
set, or autonegotiation enable (12) clear, it is rewritten without them and
with autonegotiation restarted (9).

## Descriptors (16 bytes)

Receive, advanced one-buffer format. The driver writes the packet buffer
address (0–7) and a zero header buffer address (8–15: no header split).
The controller writes back packet type and RSS information (0–7), the
extended status/error word (8–11: DD bit 0 done, EOP bit 1 end of packet;
frame errors CRC 24, symbol 25, sequence 26, carrier extension 28, RX data
31; the IP/L4 checksum error bits are only offload results), the length
(12–13) and the VLAN tag (14–15). DD lands in bit 0 of the old header
address, which is why that field must be zero when the descriptor is
posted. The legacy receive format (SRRCTL type 000) is not used: QEMU's
82576 model writes the advanced format whatever SRRCTL says.

Transmit, legacy format: buffer address (0–7), length (8–9), checksum offset (10),
command (11: EOP 0x01, insert FCS 0x02, report status 0x08), status (12:
DD 0x01), checksum start (13), VLAN (14–15).

The controller owns receive descriptors RDH…RDT − 1 and transmit
descriptors TDH…TDT − 1. RDT = RDH means no buffer (frames are dropped,
with SRRCTL.DROP_EN, not paused); TDT = TDH means nothing to send, so at
most count − 1 frames are in flight.
