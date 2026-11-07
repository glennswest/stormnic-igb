//! The Intel igb MACs (82575, 82576, 82580, I350, I354, I210, I211) and the
//! internal PHY's standard registers, independent of UEFI so every sequence
//! can be tested against a simulated controller.
//!
//! Hardware facts (register offsets, bit fields, the reset and queue-enable
//! sequences, the legacy descriptor formats) are from Intel's public
//! datasheets for the 82576, I350 and I210; docs/hardware.md describes them
//! in its own words. The code is written here. What this driver deliberately
//! does not do:
//!
//! - no interrupts (the SNP polls), no RSS or VMDq, one queue, no offloads,
//!   no VLAN, no jumbo frames;
//! - **no 802.3x flow control**: CTRL.TFCE/RFCE stay clear, the XON/XOFF
//!   thresholds are zero and SRRCTL.DROP_EN is set, so a receive ring nobody
//!   drains drops frames instead of pausing the switch (stormnic-realtek#2);
//! - no PHY reset: the link the firmware or power-on brought up stays. Start
//!   only powers the internal PHY up and turns autonegotiation on if a
//!   previous driver left it off, and only while it holds the software/
//!   firmware PHY semaphore (a BMC can share the port).
//!
//! The caller reaches the registers through `Bus` (memory BAR 0) and
//! supplies the firmware's Stall. `ring` (the descriptor rings) and `snp`
//! (the SNP state machine) build on this.

#[path = "barrier.rs"]
pub mod barrier;
#[path = "ring.rs"]
pub mod ring;
#[path = "snp_core.rs"]
pub mod snp;

/// PCI vendor of every controller here.
pub const VENDOR: u16 = 0x8086;

// Registers (memory BAR 0).
pub const CTRL: u32 = 0x0000;
pub const STATUS: u32 = 0x0008;
pub const EECD: u32 = 0x0010;
pub const CTRL_EXT: u32 = 0x0018;
pub const MDIC: u32 = 0x0020;
pub const ICR: u32 = 0x00c0;
pub const IMC: u32 = 0x00d8;
pub const RCTL: u32 = 0x0100;
pub const FCTTV: u32 = 0x0170;
pub const TCTL: u32 = 0x0400;
pub const EIMC: u32 = 0x1528;
pub const FCRTL: u32 = 0x2160;
pub const FCRTH: u32 = 0x2168;
pub const MTA: u32 = 0x5200;
pub const MTA_COUNT: u32 = 128;
pub const RAL0: u32 = 0x5400;
pub const RAH0: u32 = 0x5404;
pub const MRQC: u32 = 0x5818;
pub const SWSM: u32 = 0x5b50;
pub const SW_FW_SYNC: u32 = 0x5b5c;
// Receive queue 0.
pub const RDBAL: u32 = 0xc000;
pub const RDBAH: u32 = 0xc004;
pub const RDLEN: u32 = 0xc008;
pub const SRRCTL: u32 = 0xc00c;
pub const RDH: u32 = 0xc010;
pub const RDT: u32 = 0xc018;
pub const RXDCTL: u32 = 0xc028;
// Transmit queue 0.
pub const TDBAL: u32 = 0xe000;
pub const TDBAH: u32 = 0xe004;
pub const TDLEN: u32 = 0xe008;
pub const TDH: u32 = 0xe010;
pub const TDT: u32 = 0xe018;
pub const TXDCTL: u32 = 0xe028;

// CTRL.
pub const CTRL_GIO_MASTER_DISABLE: u32 = 1 << 2;
pub const CTRL_SLU: u32 = 1 << 6;
pub const CTRL_ILOS: u32 = 1 << 7;
pub const CTRL_FRCSPD: u32 = 1 << 11;
pub const CTRL_FRCDPLX: u32 = 1 << 12;
pub const CTRL_RST: u32 = 1 << 26;
pub const CTRL_RFCE: u32 = 1 << 27;
pub const CTRL_TFCE: u32 = 1 << 28;
pub const CTRL_VME: u32 = 1 << 30;
pub const CTRL_PHY_RST: u32 = 1 << 31;
// STATUS.
pub const STATUS_FD: u32 = 1 << 0;
pub const STATUS_LU: u32 = 1 << 1;
pub const STATUS_FUNC_SHIFT: u32 = 2;
pub const STATUS_FUNC_MASK: u32 = 0x3 << STATUS_FUNC_SHIFT;
pub const STATUS_SPEED_SHIFT: u32 = 6;
pub const STATUS_SPEED_MASK: u32 = 0x3 << STATUS_SPEED_SHIFT;
pub const STATUS_GIO_MASTER_ENABLE: u32 = 1 << 19;
// EECD: the NVM's contents have been loaded after a reset.
pub const EECD_AUTO_RD: u32 = 1 << 9;
// CTRL_EXT.
pub const CTRL_EXT_LINK_MODE_SHIFT: u32 = 22;
pub const CTRL_EXT_LINK_MODE_MASK: u32 = 0x3 << CTRL_EXT_LINK_MODE_SHIFT;
pub const CTRL_EXT_DRV_LOAD: u32 = 1 << 28;
// MDIC.
pub const MDIC_REG_SHIFT: u32 = 16;
pub const MDIC_PHY_SHIFT: u32 = 21;
pub const MDIC_OP_WRITE: u32 = 1 << 26;
pub const MDIC_OP_READ: u32 = 2 << 26;
pub const MDIC_READY: u32 = 1 << 28;
pub const MDIC_ERROR: u32 = 1 << 30;
/// The internal PHY's MDIO address.
pub const PHY_ADDR: u32 = 1;
// RCTL.
pub const RCTL_EN: u32 = 1 << 1;
pub const RCTL_UPE: u32 = 1 << 3;
pub const RCTL_MPE: u32 = 1 << 4;
pub const RCTL_BAM: u32 = 1 << 15;
pub const RCTL_SECRC: u32 = 1 << 26;
// TCTL.
pub const TCTL_EN: u32 = 1 << 1;
pub const TCTL_PSP: u32 = 1 << 3;
pub const TCTL_CT: u32 = 0x0f << 4;
pub const TCTL_COLD: u32 = 0x3f << 12;
pub const TCTL_RTLC: u32 = 1 << 24;
// RAH.
pub const RAH_AV: u32 = 1 << 31;
// SRRCTL: packet buffer size in KB (descriptor type field 0: legacy), drop
// when no descriptor is free.
pub const SRRCTL_BSIZE_2K: u32 = 2;
pub const SRRCTL_DROP_EN: u32 = 1 << 31;
// RXDCTL / TXDCTL.
pub const QUEUE_ENABLE: u32 = 1 << 25;
pub const WTHRESH_1: u32 = 1 << 16;
// SWSM / SW_FW_SYNC.
pub const SWSM_SMBI: u32 = 1 << 0;
pub const SWSM_SWESMBI: u32 = 1 << 1;
/// SW_FW_SYNC's firmware-owned half is the software half shifted up 16.
pub const SYNC_FW_SHIFT: u32 = 16;

// Standard MII registers of the PHY, and their bits.
pub const MII_BMCR: u32 = 0;
pub const BMCR_RESTART_AN: u16 = 1 << 9;
pub const BMCR_ISOLATE: u16 = 1 << 10;
pub const BMCR_PDOWN: u16 = 1 << 11;
pub const BMCR_ANENABLE: u16 = 1 << 12;

/// The MAC generation, from the PCI device ID (ids.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mac {
    M82575,
    M82576,
    M82580,
    I350,
    I354,
    I210,
    I211,
}

impl Mac {
    pub fn name(self) -> &'static str {
        match self {
            Mac::M82575 => "82575",
            Mac::M82576 => "82576",
            Mac::M82580 => "82580",
            Mac::I350 => "I350",
            Mac::I354 => "I354",
            Mac::I210 => "I210",
            Mac::I211 => "I211",
        }
    }

    /// The SW_FW_SYNC bit for this function's PHY (`func` from STATUS).
    pub fn phy_sync_bit(func: u32) -> u32 {
        match func & 3 {
            0 => 0x02,
            1 => 0x04,
            2 => 0x20,
            _ => 0x40,
        }
    }
}

impl core::fmt::Display for Mac {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { f.write_str(self.name()) }
}

/// The link interface CTRL_EXT.LINK_MODE selects (loaded from the NVM).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    /// The internal copper PHY, on MDIO address 1.
    Copper,
    /// 1000BASE-KX backplane.
    Kx,
    /// SGMII to an external PHY.
    Sgmii,
    /// SerDes (fiber or backplane).
    Serdes,
}

impl Media {
    pub fn from_ctrl_ext(v: u32) -> Media {
        match (v & CTRL_EXT_LINK_MODE_MASK) >> CTRL_EXT_LINK_MODE_SHIFT {
            0 => Media::Copper,
            1 => Media::Kx,
            2 => Media::Sgmii,
            _ => Media::Serdes,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Media::Copper => "copper",
            Media::Kx => "1000BASE-KX",
            Media::Sgmii => "SGMII",
            Media::Serdes => "SerDes",
        }
    }
}

/// The memory BAR's registers and the firmware's Stall.
pub trait Bus {
    type Error: Copy + core::fmt::Debug;
    fn r32(&mut self, offset: u32) -> Result<u32, Self::Error>;
    fn w32(&mut self, offset: u32, value: u32) -> Result<(), Self::Error>;
    fn delay_us(&mut self, micros: usize);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    Io(E),
    /// STATUS reads all ones: the function is gone (or its BAR is not decoded).
    Removed,
    /// RAL0/RAH0 hold no valid unicast address.
    NoMac,
    /// CTRL.RST did not clear within 20 ms.
    ResetTimeout { ctrl: u32 },
    /// A queue's enable bit did not follow the write within 10 ms.
    QueueTimeout { tx: bool, value: u32 },
    /// An MDIC access did not complete, or the PHY answered with an error.
    Mdic { reg: u32, value: u32 },
    /// A frame outside 14..=1518 bytes to send.
    Frame { len: usize },
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self { Error::Io(e) }
}

pub type R<T, E> = Result<T, Error<E>>;

/// Link state, from STATUS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Link {
    pub up: bool,
    pub mbps: u32,
    pub full_duplex: bool,
}

impl Link {
    pub fn from_status(s: u32) -> Link {
        let mbps = match (s & STATUS_SPEED_MASK) >> STATUS_SPEED_SHIFT {
            0 => 10,
            1 => 100,
            _ => 1000,
        };
        let up = s & STATUS_LU != 0;
        Link { up, mbps: if up { mbps } else { 0 }, full_duplex: up && s & STATUS_FD != 0 }
    }
}

impl core::fmt::Display for Link {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if !self.up { return f.write_str("down"); }
        write!(f, "up {} Mb/s {}", self.mbps, if self.full_duplex { "full duplex" } else { "half duplex" })
    }
}

/// What `identify` reads without changing anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub status: u32,
    /// The LAN function (port) number, STATUS.FUNC_ID.
    pub func: u32,
    pub media: Media,
    /// The receive-address register's address, if it holds a valid one.
    pub mac: Option<[u8; 6]>,
    pub link: Link,
}

pub fn unicast(m: [u8; 6]) -> bool {
    m != [0; 6] && m != [0xff; 6] && m[0] & 1 == 0
}

/// STATUS, the media, the station address and the link, read only.
pub fn identify<B: Bus>(b: &mut B) -> R<Identity, B::Error> {
    let status = b.r32(STATUS)?;
    if status == u32::MAX { return Err(Error::Removed); }
    let media = Media::from_ctrl_ext(b.r32(CTRL_EXT)?);
    let mac = read_mac(b)?;
    Ok(Identity {
        status,
        func: (status & STATUS_FUNC_MASK) >> STATUS_FUNC_SHIFT,
        media,
        mac,
        link: Link::from_status(status),
    })
}

/// RAL0/RAH0: the address the hardware loaded from the NVM at reset (or
/// one a previous driver wrote), when RAH.AV is set and it is unicast.
pub fn read_mac<B: Bus>(b: &mut B) -> R<Option<[u8; 6]>, B::Error> {
    let lo = b.r32(RAL0)?.to_le_bytes();
    let hi = b.r32(RAH0)?;
    let h = hi.to_le_bytes();
    let m = [lo[0], lo[1], lo[2], lo[3], h[0], h[1]];
    Ok((hi & RAH_AV != 0 && unicast(m)).then_some(m))
}

pub fn link<B: Bus>(b: &mut B) -> R<Link, B::Error> {
    Ok(Link::from_status(b.r32(STATUS)?))
}

fn set32<B: Bus>(b: &mut B, r: u32, bits: u32) -> R<(), B::Error> { let v = b.r32(r)?; Ok(b.w32(r, v | bits)?) }
fn clr32<B: Bus>(b: &mut B, r: u32, bits: u32) -> R<(), B::Error> { let v = b.r32(r)?; Ok(b.w32(r, v & !bits)?) }

/// Poll `f` every `step` µs, up to `tries` times; whether it came true.
fn wait<B: Bus>(b: &mut B, tries: usize, step: usize, mut f: impl FnMut(&mut B) -> R<bool, B::Error>) -> R<bool, B::Error> {
    for _ in 0..tries {
        if f(b)? { return Ok(true); }
        b.delay_us(step);
    }
    f(b)
}

/// What a reset saw, for the trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Reset {
    /// Bus-master requests drained (STATUS.GIO_MASTER_ENABLE cleared) before
    /// the reset; false: it was reset with requests still pending.
    pub master_drained: bool,
    /// EECD.AUTO_RD set after it: the NVM was reloaded.
    pub nvm_loaded: bool,
}

/// Interrupts masked (legacy and extended) and the cause register read
/// clear.
fn mask_interrupts<B: Bus>(b: &mut B) -> R<(), B::Error> {
    b.w32(IMC, u32::MAX)?;
    b.w32(EIMC, u32::MAX)?;
    b.r32(ICR)?;
    Ok(())
}

/// Software reset of the MAC (not the PHY): bus-master requests drained
/// first, then CTRL.RST, which clears itself; then the NVM reload. After it
/// the controller does no DMA, RX/TX and both queues are off and RAL0/RAH0
/// hold the NVM's address again.
pub fn reset<B: Bus>(b: &mut B) -> R<Reset, B::Error> {
    set32(b, CTRL, CTRL_GIO_MASTER_DISABLE)?;
    let master_drained = wait(b, 80, 100, |b| Ok(b.r32(STATUS)? & STATUS_GIO_MASTER_ENABLE == 0))?;
    let ctrl = b.r32(CTRL)?;
    b.w32(CTRL, ctrl | CTRL_RST)?;
    b.delay_us(1000);
    if !wait(b, 190, 100, |b| Ok(b.r32(CTRL)? & CTRL_RST == 0))? {
        return Err(Error::ResetTimeout { ctrl: b.r32(CTRL)? });
    }
    let nvm_loaded = wait(b, 100, 100, |b| Ok(b.r32(EECD)? & EECD_AUTO_RD != 0))?;
    mask_interrupts(b)?;
    clr32(b, CTRL_EXT, CTRL_EXT_DRV_LOAD)?;
    Ok(Reset { master_drained, nvm_loaded })
}

/// Quiesce (stormnic-realtek#2): interrupts masked, receive and transmit
/// off, both queues disabled, then `reset`. Every path that leaves the NIC
/// calls this.
pub fn quiesce<B: Bus>(b: &mut B) -> R<Reset, B::Error> {
    mask_interrupts(b)?;
    clr32(b, RCTL, RCTL_EN)?;
    clr32(b, RXDCTL, QUEUE_ENABLE)?;
    clr32(b, TXDCTL, QUEUE_ENABLE)?;
    clr32(b, TCTL, TCTL_EN)?;
    // Let a frame in flight finish; the reset below stops the rest.
    wait(b, 100, 100, |b| Ok(b.r32(RXDCTL)? & QUEUE_ENABLE == 0 && b.r32(TXDCTL)? & QUEUE_ENABLE == 0))?;
    reset(b)
}

/// Where `mac_init` points the controller.
#[derive(Clone, Copy, Debug)]
pub struct Rings {
    pub rx: u64,
    pub rx_bytes: u32,
    pub tx: u64,
    pub tx_bytes: u32,
}

/// Program the MAC after `reset`: link up without forced speed or flow
/// control, the station address, no multicast hash or RSS, both rings with
/// their queues enabled, the transmitter on. Receive stays off (`enable`
/// turns it on once the buffers are posted); RDT is the caller's.
pub fn mac_init<B: Bus>(b: &mut B, mac: [u8; 6], rings: Rings) -> R<(), B::Error> {
    let ctrl = b.r32(CTRL)?;
    let off = CTRL_FRCSPD | CTRL_FRCDPLX | CTRL_RFCE | CTRL_TFCE | CTRL_VME | CTRL_PHY_RST | CTRL_ILOS
        | CTRL_GIO_MASTER_DISABLE | CTRL_RST;
    b.w32(CTRL, (ctrl & !off) | CTRL_SLU)?;
    b.w32(FCRTL, 0)?;
    b.w32(FCRTH, 0)?;
    b.w32(FCTTV, 0)?;
    b.w32(RAL0, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]))?;
    b.w32(RAH0, u32::from(mac[4]) | (u32::from(mac[5]) << 8) | RAH_AV)?;
    for i in 0..MTA_COUNT { b.w32(MTA + 4 * i, 0)?; }
    b.w32(MRQC, 0)?;
    b.w32(RCTL, 0)?;
    // Receive queue 0: legacy descriptors, 2 KB buffers, drop when full.
    b.w32(RXDCTL, 0)?;
    b.w32(RDBAL, rings.rx as u32)?;
    b.w32(RDBAH, (rings.rx >> 32) as u32)?;
    b.w32(RDLEN, rings.rx_bytes)?;
    b.w32(RDH, 0)?;
    b.w32(RDT, 0)?;
    b.w32(SRRCTL, SRRCTL_BSIZE_2K | SRRCTL_DROP_EN)?;
    b.w32(RXDCTL, QUEUE_ENABLE | WTHRESH_1)?;
    if !wait(b, 100, 100, |b| Ok(b.r32(RXDCTL)? & QUEUE_ENABLE != 0))? {
        return Err(Error::QueueTimeout { tx: false, value: b.r32(RXDCTL)? });
    }
    // Transmit queue 0.
    b.w32(TXDCTL, 0)?;
    b.w32(TDBAL, rings.tx as u32)?;
    b.w32(TDBAH, (rings.tx >> 32) as u32)?;
    b.w32(TDLEN, rings.tx_bytes)?;
    b.w32(TDH, 0)?;
    b.w32(TDT, 0)?;
    b.w32(TXDCTL, QUEUE_ENABLE | WTHRESH_1)?;
    if !wait(b, 100, 100, |b| Ok(b.r32(TXDCTL)? & QUEUE_ENABLE != 0))? {
        return Err(Error::QueueTimeout { tx: true, value: b.r32(TXDCTL)? });
    }
    b.w32(TCTL, TCTL_EN | TCTL_PSP | TCTL_CT | TCTL_COLD | TCTL_RTLC)?;
    set32(b, CTRL_EXT, CTRL_EXT_DRV_LOAD)?;
    Ok(())
}

/// RCTL for the filter: station address, broadcast and every multicast (the
/// SNP filters further in software), everything with `promiscuous`; CRC
/// stripped, 2 KB buffers, no long frames, no VLAN filter.
pub fn rctl(promiscuous: bool) -> u32 {
    RCTL_EN | RCTL_BAM | RCTL_MPE | RCTL_SECRC | if promiscuous { RCTL_UPE } else { 0 }
}

/// Turn receive on, after the buffers are posted.
pub fn enable<B: Bus>(b: &mut B, promiscuous: bool) -> R<(), B::Error> {
    Ok(b.w32(RCTL, rctl(promiscuous))?)
}

pub fn set_filter<B: Bus>(b: &mut B, promiscuous: bool) -> R<(), B::Error> {
    let en = b.r32(RCTL)? & RCTL_EN;
    Ok(b.w32(RCTL, (rctl(promiscuous) & !RCTL_EN) | en)?)
}

// The software/firmware semaphore around the PHY.

/// Take SWSM: SMBI (set by the hardware on the read that finds it clear),
/// then SWESMBI (only sticks when firmware does not hold it). Whether it was
/// taken; on false nothing is held.
fn take_swsm<B: Bus>(b: &mut B) -> R<bool, B::Error> {
    if !wait(b, 200, 50, |b| Ok(b.r32(SWSM)? & SWSM_SMBI == 0))? { return Ok(false); }
    for _ in 0..200 {
        let v = b.r32(SWSM)?;
        b.w32(SWSM, v | SWSM_SWESMBI)?;
        if b.r32(SWSM)? & SWSM_SWESMBI != 0 { return Ok(true); }
        b.delay_us(50);
    }
    give_swsm(b)?;
    Ok(false)
}

fn give_swsm<B: Bus>(b: &mut B) -> R<(), B::Error> { clr32(b, SWSM, SWSM_SMBI | SWSM_SWESMBI) }

/// Why the PHY was left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhySkip {
    /// No internal PHY: the media is not copper.
    NotCopper(Media),
    /// The semaphore could not be had in about 1 s (firmware holds the PHY,
    /// or the controller does not implement the semaphore). SW_FW_SYNC as
    /// last read; SWSM is not read for this, since a read that finds SMBI
    /// clear takes it.
    Busy { sync: u32 },
}

/// Run `f` holding this function's PHY bit in SW_FW_SYNC. Err(PhySkip::Busy)
/// if it could not be had.
pub fn with_phy<B: Bus, T>(b: &mut B, func: u32, f: impl FnOnce(&mut B) -> R<T, B::Error>)
    -> R<Result<T, PhySkip>, B::Error> {
    let bit = Mac::phy_sync_bit(func);
    let mut got = false;
    let mut sync = 0;
    for _ in 0..100 {
        if take_swsm(b)? {
            sync = b.r32(SW_FW_SYNC)?;
            if sync & (bit | (bit << SYNC_FW_SHIFT)) == 0 {
                b.w32(SW_FW_SYNC, sync | bit)?;
                got = b.r32(SW_FW_SYNC)? & bit != 0;
                give_swsm(b)?;
                if got { break; }
            } else {
                give_swsm(b)?;
            }
        }
        b.delay_us(5000);
    }
    if !got {
        return Ok(Err(PhySkip::Busy { sync }));
    }
    let r = f(b);
    // Give the bit back whatever `f` did.
    if take_swsm(b)? {
        clr32(b, SW_FW_SYNC, bit)?;
        give_swsm(b)?;
    } else {
        clr32(b, SW_FW_SYNC, bit)?;
    }
    r.map(Ok)
}

/// One MDIC access to the internal PHY; the data read.
fn mdic<B: Bus>(b: &mut B, op: u32, reg: u32, data: u16) -> R<u16, B::Error> {
    b.w32(MDIC, op | (PHY_ADDR << MDIC_PHY_SHIFT) | (reg << MDIC_REG_SHIFT) | u32::from(data))?;
    let mut v = 0;
    let done = wait(b, 1000, 50, |b| { v = b.r32(MDIC)?; Ok(v & MDIC_READY != 0) })?;
    if !done || v & MDIC_ERROR != 0 { return Err(Error::Mdic { reg, value: v }); }
    Ok(v as u16)
}

pub fn mii_r<B: Bus>(b: &mut B, reg: u32) -> R<u16, B::Error> { mdic(b, MDIC_OP_READ, reg, 0) }
pub fn mii_w<B: Bus>(b: &mut B, reg: u32, v: u16) -> R<(), B::Error> { mdic(b, MDIC_OP_WRITE, reg, v).map(|_| ()) }

/// What `phy_up` found and did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phy {
    /// BMCR already had the PHY powered, not isolated, autonegotiating.
    Fine { bmcr: u16 },
    /// BMCR was rewritten and autonegotiation restarted: the link takes a
    /// few seconds.
    Restarted { before: u16, after: u16 },
    Skipped(PhySkip),
}

/// The BMCR `phy_up` wants: powered, not isolated, autonegotiating.
pub fn wanted_bmcr(bmcr: u16) -> u16 {
    (bmcr & !(BMCR_PDOWN | BMCR_ISOLATE)) | BMCR_ANENABLE
}

/// Make sure the internal PHY is powered up and autonegotiating, without
/// resetting it: only BMCR is touched, and only if a previous driver left it
/// powered down, isolated or with autonegotiation off.
pub fn phy_up<B: Bus>(b: &mut B, id: &Identity) -> R<Phy, B::Error> {
    if id.media != Media::Copper { return Ok(Phy::Skipped(PhySkip::NotCopper(id.media))); }
    let r = with_phy(b, id.func, |b| {
        let bmcr = mii_r(b, MII_BMCR)?;
        let want = wanted_bmcr(bmcr);
        if want == bmcr { return Ok(Phy::Fine { bmcr }); }
        mii_w(b, MII_BMCR, want | BMCR_RESTART_AN)?;
        Ok(Phy::Restarted { before: bmcr, after: want })
    })?;
    Ok(r.unwrap_or_else(Phy::Skipped))
}

/// Wait up to `ms` for the link; the last state read.
pub fn wait_link<B: Bus>(b: &mut B, ms: usize) -> R<Link, B::Error> {
    let mut l = link(b)?;
    for _ in 0..ms / 10 {
        if l.up { break; }
        b.delay_us(10_000);
        l = link(b)?;
    }
    Ok(l)
}
