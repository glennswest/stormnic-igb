//! A simulated igb controller that DMAs, shared by the host tests: a 32-bit
//! register file with the registers that act (CTRL's reset and GIO master
//! disable, STATUS from a modelled link, EECD's NVM-loaded bit, MDIC to a
//! PHY register file, SWSM and SW_FW_SYNC with a firmware that can hold the
//! PHY, the queue-enable bits, ICR read-to-clear), and queue 0's legacy
//! descriptor rings walked by head and tail the way docs/hardware.md
//! describes.
use crate::igb::ring::{self, Dma, Net, DMA_BYTES};
use crate::igb::{self, Bus};
use std::collections::HashMap;

pub const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
/// Where the simulated IOMMU puts the region.
pub const DEVICE_BASE: u64 = 0x1_2345_0000;
/// Autonegotiation time after a restart.
pub const AN_US: u64 = 2_000_000;
/// After a MAC reset the MAC's side of the link is back after this.
pub const RELINK_US: u64 = 50_000;
/// A typical BMCR: autonegotiation on, full duplex, 1000 Mb/s selected.
pub const BMCR_DEFAULT: u16 = 0x1140;

pub struct Dev {
    base: *mut u8,
    pub regs: HashMap<u32, u32>,
    pub phy: HashMap<u32, u16>,
    /// The NVM's station address, put into RAL0/RAH0 at every reset.
    pub nvm_mac: Option<[u8; 6]>,
    /// Microseconds of Stall so far.
    pub now: u64,
    pub cable: bool,
    an_done_at: u64,
    pub an_restarts: usize,
    /// The MAC's side of the link comes back at this time (after a reset).
    relink_at: u64,
    pub resets: usize,
    /// CTRL.RST never clears.
    pub reset_stuck: bool,
    /// STATUS.GIO_MASTER_ENABLE never clears.
    pub master_stuck: bool,
    /// Queue enable bits never read back set.
    pub queue_stuck: bool,
    /// MDIC never completes.
    pub mdic_stuck: bool,
    /// The firmware holds this function's PHY bit in SW_FW_SYNC.
    pub fw_holds_phy: bool,
    /// PHY register accesses made while software held the PHY bit / not.
    pub phy_locked_accesses: usize,
    pub phy_unlocked_accesses: usize,
    /// QEMU's model: after a MAC reset STATUS.LU stays down until an
    /// autonegotiation ends (real controllers see the PHY's link again).
    pub qemu_relink: bool,
    /// The LAN function (STATUS.FUNC_ID), which picks the PHY's sync bit.
    pub function: u32,
    tx_head: u32,
    rx_head: u32,
    pub wire: Vec<Vec<u8>>,
    pub loopback: bool,
    pub dropped: usize,
    /// Every register write as (offset, value).
    pub ops: Vec<(u32, u32)>,
}

impl Dev {
    pub fn new() -> Dev {
        let layout = std::alloc::Layout::from_size_align(DMA_BYTES, 4096).unwrap();
        // SAFETY: a non-zero layout; the memory is leaked for the test.
        let base = unsafe { std::alloc::alloc_zeroed(layout) };
        let mut d = Dev {
            base,
            regs: HashMap::new(),
            phy: HashMap::new(),
            nvm_mac: Some(MAC),
            now: 0,
            cable: true,
            an_done_at: 0,
            an_restarts: 0,
            relink_at: 0,
            resets: 0,
            reset_stuck: false,
            master_stuck: false,
            queue_stuck: false,
            mdic_stuck: false,
            fw_holds_phy: false,
            qemu_relink: false,
            phy_locked_accesses: 0,
            phy_unlocked_accesses: 0,
            function: 0,
            tx_head: 0,
            rx_head: 0,
            wire: Vec::new(),
            loopback: false,
            dropped: 0,
            ops: Vec::new(),
        };
        d.phy.insert(igb::MII_BMCR, BMCR_DEFAULT);
        d.power_on();
        // Power-on: the link is already up (only a later MAC reset drops it).
        d.relink_at = 0;
        // What a firmware driver typically leaves: link up, flow control on.
        d.put(igb::CTRL, igb::CTRL_SLU | igb::CTRL_RFCE | igb::CTRL_TFCE);
        d.put(igb::FCRTH, 0x8000_3000);
        d
    }

    /// The registers' reset values (power-on and CTRL.RST): RX/TX and both
    /// queues off, the NVM's address loaded.
    fn power_on(&mut self) {
        let keep_fw = self.get(igb::SW_FW_SYNC) & 0xffff_0000;
        let sw = self.get(igb::SW_FW_SYNC) & 0xffff;
        self.regs.clear();
        self.put(igb::EECD, igb::EECD_AUTO_RD);
        self.put(igb::TCTL, igb::TCTL_PSP);
        // The semaphore is not reset by a MAC reset.
        self.put(igb::SW_FW_SYNC, keep_fw | sw);
        if let Some(m) = self.nvm_mac {
            self.put(igb::RAL0, u32::from_le_bytes([m[0], m[1], m[2], m[3]]));
            self.put(igb::RAH0, u32::from(m[4]) | u32::from(m[5]) << 8 | igb::RAH_AV);
        }
        self.tx_head = 0;
        self.rx_head = 0;
        self.relink_at = if self.qemu_relink { u64::MAX } else { self.now + RELINK_US };
    }

    pub fn dma(&self) -> Dma { Dma { host: self.base, device: DEVICE_BASE } }

    /// A Net over this device's region.
    pub fn net(&self) -> Net {
        // SAFETY: the region is DMA_BYTES, leaked, used only by this Net and
        // the simulated DMA below.
        unsafe { Net::new(self.dma(), MAC) }
    }

    pub fn get(&self, r: u32) -> u32 { self.regs.get(&r).copied().unwrap_or(0) }
    fn put(&mut self, r: u32, v: u32) { self.regs.insert(r, v); }

    fn host(&self, device: u64, len: usize) -> *mut u8 {
        let off = device.checked_sub(DEVICE_BASE).expect("DMA below the region") as usize;
        assert!(off + len <= DMA_BYTES, "DMA past the region: {off:#x}+{len}");
        // SAFETY: inside the region (checked).
        unsafe { self.base.add(off) }
    }
    fn mem8(&self, device: u64) -> u8 {
        // SAFETY: inside the region.
        unsafe { std::ptr::read_volatile(self.host(device, 1)) }
    }
    fn set_mem8(&self, device: u64, v: u8) {
        // SAFETY: inside the region.
        unsafe { std::ptr::write_volatile(self.host(device, 1), v) }
    }
    pub fn mem32(&self, device: u64) -> u32 {
        // SAFETY: inside the region.
        u32::from_le(unsafe { std::ptr::read_volatile(self.host(device, 4) as *const u32) })
    }
    fn set_mem16(&self, device: u64, v: u16) {
        // SAFETY: inside the region.
        unsafe { std::ptr::write_volatile(self.host(device, 2) as *mut u16, v.to_le()) }
    }
    fn mem64(&self, device: u64) -> u64 { u64::from(self.mem32(device)) | u64::from(self.mem32(device + 4)) << 32 }

    pub fn rx_ring(&self) -> u64 { u64::from(self.get(igb::RDBAL)) | u64::from(self.get(igb::RDBAH)) << 32 }
    pub fn tx_ring(&self) -> u64 { u64::from(self.get(igb::TDBAL)) | u64::from(self.get(igb::TDBAH)) << 32 }
    fn rx_count(&self) -> u32 { self.get(igb::RDLEN) / 16 }
    fn tx_count(&self) -> u32 { self.get(igb::TDLEN) / 16 }

    pub fn rx_enabled(&self) -> bool {
        self.get(igb::RCTL) & igb::RCTL_EN != 0 && self.get(igb::RXDCTL) & igb::QUEUE_ENABLE != 0
    }
    pub fn tx_enabled(&self) -> bool {
        self.get(igb::TCTL) & igb::TCTL_EN != 0 && self.get(igb::TXDCTL) & igb::QUEUE_ENABLE != 0
    }

    pub fn link_up(&self) -> bool {
        self.cable && self.now >= self.an_done_at && self.now >= self.relink_at
            && self.get(igb::CTRL) & igb::CTRL_SLU != 0
            && self.phy.get(&igb::MII_BMCR).copied().unwrap_or(0) & igb::BMCR_PDOWN == 0
    }

    fn status(&self) -> u32 {
        let mut s = self.function << igb::STATUS_FUNC_SHIFT;
        if self.get(igb::CTRL) & igb::CTRL_GIO_MASTER_DISABLE == 0 || self.master_stuck {
            s |= igb::STATUS_GIO_MASTER_ENABLE;
        }
        if self.link_up() { s |= igb::STATUS_LU | igb::STATUS_FD | 2 << igb::STATUS_SPEED_SHIFT; }
        s
    }

    fn sw_holds_phy(&self) -> bool {
        self.get(igb::SW_FW_SYNC) & igb::Mac::phy_sync_bit(self.function) != 0
    }

    fn mdic_write(&mut self, v: u32) {
        if self.mdic_stuck { self.put(igb::MDIC, v & !igb::MDIC_READY); return; }
        let reg = (v >> igb::MDIC_REG_SHIFT) & 0x1f;
        let phy = (v >> igb::MDIC_PHY_SHIFT) & 0x1f;
        if self.sw_holds_phy() { self.phy_locked_accesses += 1; } else { self.phy_unlocked_accesses += 1; }
        if phy != igb::PHY_ADDR {
            self.put(igb::MDIC, v | igb::MDIC_READY | igb::MDIC_ERROR);
            return;
        }
        let data = if v & igb::MDIC_OP_READ != 0 && reg == igb::MII_BMSR {
            // 10/100/1000 abilities, autonegotiation able; link from the cable.
            let pdown = self.phy.get(&igb::MII_BMCR).copied().unwrap_or(0) & igb::BMCR_PDOWN != 0;
            0x7949 | if self.cable && !pdown { igb::BMSR_LINK } else { 0 }
        } else if v & igb::MDIC_OP_READ != 0 {
            self.phy.get(&reg).copied().unwrap_or(0)
        } else {
            let mut w = v as u16;
            if reg == igb::MII_BMCR && w & igb::BMCR_RESTART_AN != 0 {
                self.an_restarts += 1;
                self.an_done_at = self.now + AN_US;
                self.relink_at = 0;
                w &= !igb::BMCR_RESTART_AN;
            }
            self.phy.insert(reg, w);
            v as u16
        };
        self.put(igb::MDIC, (v & 0xffff_0000) | u32::from(data) | igb::MDIC_READY);
    }

    fn ctrl_write(&mut self, v: u32) {
        if v & igb::CTRL_RST != 0 {
            self.resets += 1;
            assert!(self.status() & igb::STATUS_GIO_MASTER_ENABLE == 0 || self.master_stuck,
                "reset with bus-master requests not drained");
            self.power_on();
            self.put(igb::CTRL, if self.reset_stuck { igb::CTRL_RST } else { 0 });
            return;
        }
        self.put(igb::CTRL, v);
    }

    /// TDT moved: send every descriptor from the head up to the tail.
    fn transmit(&mut self) {
        if !self.tx_enabled() { return; }
        let ring = self.tx_ring();
        let n = self.tx_count();
        while self.tx_head != self.get(igb::TDT) {
            let d = ring + 16 * u64::from(self.tx_head);
            let lenw = self.mem32(d + 8);
            let cmd = (lenw >> 24) as u8;
            assert_eq!(cmd & (ring::TX_EOP | ring::TX_IFCS), ring::TX_EOP | ring::TX_IFCS, "one buffer per frame, CRC by the MAC");
            assert!(cmd & ring::TX_RS != 0, "no status report asked for");
            let len = (lenw & 0xffff) as usize;
            let addr = self.mem64(d);
            let p = self.host(addr, len);
            // SAFETY: inside the region (checked by `host`).
            let f = unsafe { std::slice::from_raw_parts(p, len) }.to_vec();
            self.set_mem8(d + 12, ring::DD);
            self.tx_head = (self.tx_head + 1) % n;
            self.put(igb::TDH, self.tx_head);
            if self.loopback { self.deliver(&f); }
            self.wire.push(f);
        }
    }

    /// A frame from the network (the MAC strips the CRC: RCTL.SECRC).
    pub fn deliver(&mut self, f: &[u8]) {
        assert!(self.get(igb::RCTL) & igb::RCTL_SECRC != 0, "CRC not stripped");
        if !self.rx_enabled() || self.rx_head == self.get(igb::RDT) {
            self.dropped += 1;
            return;
        }
        let d = self.rx_ring() + 16 * u64::from(self.rx_head);
        assert_eq!(self.mem8(d + 12), 0, "a descriptor the controller owns has DD set");
        let addr = self.mem64(d);
        let p = self.host(addr, f.len());
        // SAFETY: inside the region.
        unsafe { std::ptr::copy_nonoverlapping(f.as_ptr(), p, f.len()); }
        self.set_mem16(d + 8, f.len() as u16);
        self.set_mem8(d + 13, 0);
        self.set_mem8(d + 12, ring::DD | ring::RX_EOP);
        self.rx_head = (self.rx_head + 1) % self.rx_count();
        self.put(igb::RDH, self.rx_head);
    }

    /// Hand back the next receive descriptor with these status and error bits.
    pub fn deliver_raw(&mut self, status: u8, errors: u8, len: u16) {
        assert!(self.rx_head != self.get(igb::RDT));
        let d = self.rx_ring() + 16 * u64::from(self.rx_head);
        self.set_mem16(d + 8, len);
        self.set_mem8(d + 13, errors);
        self.set_mem8(d + 12, status);
        self.rx_head = (self.rx_head + 1) % self.rx_count();
    }

    /// Free receive descriptors (RDH up to RDT).
    pub fn rx_free(&self) -> u32 {
        (self.get(igb::RDT) + self.rx_count() - self.rx_head) % self.rx_count().max(1)
    }

    fn write(&mut self, offset: u32, v: u32) {
        self.ops.push((offset, v));
        match offset {
            igb::CTRL => self.ctrl_write(v),
            igb::MDIC => self.mdic_write(v),
            igb::TDT => { self.put(igb::TDT, v); self.transmit(); }
            igb::RXDCTL | igb::TXDCTL => self.put(offset, if self.queue_stuck { v & !igb::QUEUE_ENABLE } else { v }),
            igb::SWSM => {
                // SMBI is cleared by software; SWESMBI only sticks when the
                // firmware does not have it (modelled as never).
                self.put(igb::SWSM, v & (igb::SWSM_SMBI | igb::SWSM_SWESMBI));
            }
            igb::SW_FW_SYNC => {
                assert!(self.get(igb::SWSM) & igb::SWSM_SWESMBI != 0, "SW_FW_SYNC written without SWSM");
                let fw = self.get(igb::SW_FW_SYNC) & 0xffff_0000;
                self.put(igb::SW_FW_SYNC, fw | (v & 0xffff));
            }
            igb::TDH | igb::RDH => { assert_eq!(v, 0, "heads are written only to reset them"); self.put(offset, v); }
            _ => self.put(offset, v),
        }
        if offset == igb::TDH { self.tx_head = 0; }
        if offset == igb::RDH { self.rx_head = 0; }
    }

    fn read(&mut self, offset: u32) -> u32 {
        match offset {
            igb::STATUS => self.status(),
            igb::ICR => { let v = self.get(igb::ICR); self.put(igb::ICR, 0); v }
            igb::SWSM => {
                // The read that finds SMBI clear sets it.
                let v = self.get(igb::SWSM);
                if v & igb::SWSM_SMBI == 0 { self.put(igb::SWSM, v | igb::SWSM_SMBI); }
                v
            }
            igb::SW_FW_SYNC => {
                let fw = if self.fw_holds_phy { igb::Mac::phy_sync_bit(self.function) << igb::SYNC_FW_SHIFT } else { 0 };
                self.get(igb::SW_FW_SYNC) | fw
            }
            _ => self.get(offset),
        }
    }

    /// Writes to `offset` so far, in order.
    pub fn writes(&self, offset: u32) -> Vec<u32> {
        self.ops.iter().filter(|o| o.0 == offset).map(|o| o.1).collect()
    }
}

impl Bus for Dev {
    type Error = ();
    fn r32(&mut self, o: u32) -> Result<u32, ()> { Ok(self.read(o)) }
    fn w32(&mut self, o: u32, v: u32) -> Result<(), ()> { self.write(o, v); Ok(()) }
    fn delay_us(&mut self, us: usize) { self.now += us as u64; }
}

/// A controller and a Net brought up on it.
pub fn started() -> (Dev, Net) {
    let mut d = Dev::new();
    let mut net = d.net();
    net.start(&mut d, false).unwrap();
    d.now += RELINK_US;
    (d, net)
}

/// A minimum-size frame to `dst`, its payload tagged with `tag`.
pub fn frame_to(dst: [u8; 6], tag: u8) -> Vec<u8> {
    let mut f = vec![0u8; 60];
    f[0..6].copy_from_slice(&dst);
    f[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 9]);
    f[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    f[14] = tag;
    f
}
