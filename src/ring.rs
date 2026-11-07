//! The receive and transmit descriptor rings of queue 0, polled, one 2 KiB
//! driver-owned buffer per descriptor.
//!
//! Legacy descriptor formats (SRRCTL.DESCTYPE 0; docs/hardware.md), 16 bytes
//! each, little-endian:
//!
//! - receive: buffer address (0-7), length (8-9), checksum (10-11), status
//!   (12: DD 0x01, EOP 0x02), errors (13), VLAN tag (14-15);
//! - transmit: buffer address (0-7), length (8-9), CSO (10), command (11:
//!   EOP 0x01, IFCS 0x02, RS 0x08), status (12: DD 0x01), CSS (13), VLAN
//!   (14-15).
//!
//! Ownership moves by the head and tail registers: the controller owns the
//! receive descriptors from RDH up to RDT - 1 and the transmit descriptors
//! from TDH up to TDT - 1, and writes DD back into each one it is done with.
//! RDT = RDH means no free receive buffer; TDT = TDH means nothing to send,
//! so at most `TX_COUNT - 1` frames are in flight.
//!
//! Region layout (`DMA_BYTES`, one PciIo common buffer): the receive ring at
//! 0 (32 x 16 B), the transmit ring at 1024 (32 x 16 B; RDLEN/TDLEN are
//! multiples of 128 and the bases 128-byte aligned), then from 4096 the 32
//! receive buffers and the 32 transmit buffers. Frames are copied in and
//! out, so no caller memory is mapped for DMA. Every access to the region is
//! volatile and little-endian, and the hand-offs are ordered by `barrier`.

use super::barrier::{dma_rmb, dma_wmb, mmio_wmb};
use super::{Bus, Error, Link, Rings, R, RDT, TDT};
use core::ptr;

pub const RX_COUNT: usize = 32;
pub const TX_COUNT: usize = 32;
pub const BUF_SIZE: usize = 2048;
pub const DESC: usize = 16;
pub const RX_RING: usize = 0;
pub const TX_RING: usize = 1024;
pub const RX_BUFS: usize = 4096;
pub const TX_BUFS: usize = RX_BUFS + RX_COUNT * BUF_SIZE;
pub const DMA_BYTES: usize = TX_BUFS + TX_COUNT * BUF_SIZE;
pub const DMA_PAGES: usize = DMA_BYTES.div_ceil(4096);

/// Descriptor byte offsets.
pub const LEN: usize = 8;
pub const STATUS: usize = 12;
pub const RX_ERRORS: usize = 13;

pub const DD: u8 = 0x01;
pub const RX_EOP: u8 = 0x02;
/// Receive errors that make the frame bad: CRC/alignment, symbol, sequence,
/// carrier extension, RX data error. (The checksum errors 0x20/0x40 are only
/// offload results; the frame itself is good.)
pub const RX_FRAME_ERRORS: u8 = 0x01 | 0x02 | 0x04 | 0x10 | 0x80;
pub const TX_EOP: u8 = 0x01;
pub const TX_IFCS: u8 = 0x02;
pub const TX_RS: u8 = 0x08;

/// Largest frame without CRC: 1514 plus a 4-byte VLAN tag.
pub const MAX_FRAME: usize = 1518;
/// Ethernet header: the smallest frame the driver will send.
pub const MIN_FRAME: usize = 14;
/// Short frames are padded to this (64 bytes on the wire with the CRC the
/// MAC appends).
pub const PAD_TO: usize = 60;

/// The DMA region: `DMA_BYTES` at `host`, which the device reaches at `device`.
#[derive(Clone, Copy, Debug)]
pub struct Dma {
    pub host: *mut u8,
    pub device: u64,
}

/// A frame's Ethernet header fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ethertype: u16,
}

pub fn frame(f: &[u8]) -> Frame {
    let mut d = [0u8; 6];
    let mut s = [0u8; 6];
    d.copy_from_slice(&f[0..6]);
    s.copy_from_slice(&f[6..12]);
    Frame { destination: d, source: s, ethertype: u16::from_be_bytes([f[12], f[13]]) }
}

pub struct Net {
    dma: Dma,
    pub mac: [u8; 6],
    /// The receive descriptor to look at next.
    rx_next: usize,
    /// Transmit descriptors handed to the controller and not yet seen back:
    /// `tx_cons` up to (not including) `tx_prod`, counted modulo 2^usize.
    tx_prod: usize,
    tx_cons: usize,
    pub running: bool,
    /// Receive descriptors that came back with an error or a frame split
    /// over several buffers, since `start`.
    pub rx_errors: usize,
}

impl Net {
    /// # Safety
    /// `dma.host` is `DMA_BYTES` of memory the device reaches at
    /// `dma.device`, used by nothing else while this `Net` lives.
    pub unsafe fn new(dma: Dma, mac: [u8; 6]) -> Net {
        Net { dma, mac, rx_next: 0, tx_prod: 0, tx_cons: 0, running: false, rx_errors: 0 }
    }

    pub fn rings(&self) -> Rings {
        Rings {
            rx: self.dma.device + RX_RING as u64,
            rx_bytes: (RX_COUNT * DESC) as u32,
            tx: self.dma.device + TX_RING as u64,
            tx_bytes: (TX_COUNT * DESC) as u32,
        }
    }

    fn r8(&self, off: usize) -> u8 {
        // SAFETY: `off` is inside the region (callers index the rings).
        unsafe { ptr::read_volatile(self.dma.host.add(off)) }
    }

    fn r16(&self, off: usize) -> u16 {
        // SAFETY: as `r8`; descriptor fields are 2-byte aligned.
        u16::from_le(unsafe { ptr::read_volatile(self.dma.host.add(off) as *const u16) })
    }

    fn w32(&self, off: usize, v: u32) {
        // SAFETY: as `r8`; 4-byte aligned.
        unsafe { ptr::write_volatile(self.dma.host.add(off) as *mut u32, v.to_le()) }
    }

    fn w64(&self, off: usize, v: u64) {
        self.w32(off, v as u32);
        self.w32(off + 4, (v >> 32) as u32);
    }

    fn rx_desc(i: usize) -> usize { RX_RING + i * DESC }
    fn tx_desc(i: usize) -> usize { TX_RING + i * DESC }

    /// Point receive descriptor `i` at its buffer, status clear.
    fn post_rx(&self, i: usize) {
        let d = Self::rx_desc(i);
        self.w64(d, self.dma.device + (RX_BUFS + i * BUF_SIZE) as u64);
        self.w32(d + 8, 0);
        self.w32(d + 12, 0);
    }

    /// The whole bring-up: `reset`, `mac_init` with these rings, every
    /// receive buffer but one handed over, receive on. On failure the
    /// controller is quiesced again and nothing runs.
    pub fn start<B: Bus>(&mut self, b: &mut B, promiscuous: bool) -> R<(), B::Error> {
        self.running = false;
        let r = self.bring_up(b, promiscuous);
        if r.is_err() {
            let _ = super::quiesce(b);
        }
        r
    }

    fn bring_up<B: Bus>(&mut self, b: &mut B, promiscuous: bool) -> R<(), B::Error> {
        super::quiesce(b)?;
        // The controller is reset: the rings are ours to rewrite.
        // SAFETY: the two rings are inside the region.
        unsafe { ptr::write_bytes(self.dma.host, 0, RX_BUFS) };
        for i in 0..RX_COUNT { self.post_rx(i); }
        for i in 0..TX_COUNT {
            self.w64(Self::tx_desc(i), self.dma.device + (TX_BUFS + i * BUF_SIZE) as u64);
        }
        self.rx_next = 0;
        self.tx_prod = 0;
        self.tx_cons = 0;
        self.rx_errors = 0;
        super::mac_init(b, self.mac, self.rings())?;
        dma_wmb();
        mmio_wmb();
        // RDH is 0: the controller owns 0..RX_COUNT-2; the last one is the gap.
        b.w32(RDT, (RX_COUNT - 1) as u32)?;
        super::enable(b, promiscuous)?;
        self.running = true;
        Ok(())
    }

    /// Quiesce the controller. Ok: it does no DMA any more.
    pub fn stop<B: Bus>(&mut self, b: &mut B) -> R<(), B::Error> {
        self.running = false;
        super::quiesce(b).map(|_| ())
    }

    pub fn link<B: Bus>(&self, b: &mut B) -> R<Link, B::Error> { super::link(b) }

    pub fn set_promiscuous<B: Bus>(&self, b: &mut B, on: bool) -> R<(), B::Error> {
        if self.running { super::set_filter(b, on)?; }
        Ok(())
    }

    /// Transmit descriptors the controller still owns.
    pub fn tx_pending(&self) -> usize { self.tx_prod.wrapping_sub(self.tx_cons) }

    /// Retire the transmit descriptors the controller has written DD into.
    pub fn reclaim(&mut self) {
        while self.tx_pending() > 0 {
            let i = self.tx_cons % TX_COUNT;
            if self.r8(Self::tx_desc(i) + STATUS) & DD == 0 { break; }
            self.tx_cons = self.tx_cons.wrapping_add(1);
        }
        dma_rmb();
    }

    /// Copy `f` into the next transmit buffer and hand it over by moving
    /// TDT. Ok(false) when every usable descriptor is still in flight.
    pub fn transmit<B: Bus>(&mut self, b: &mut B, f: &[u8]) -> R<bool, B::Error> {
        if !(MIN_FRAME..=MAX_FRAME).contains(&f.len()) { return Err(Error::Frame { len: f.len() }); }
        self.reclaim();
        if self.tx_pending() == TX_COUNT - 1 { return Ok(false); }
        let i = self.tx_prod % TX_COUNT;
        let buf = TX_BUFS + i * BUF_SIZE;
        let len = f.len().max(PAD_TO);
        // SAFETY: a 2 KiB buffer inside the region that the controller does
        // not own (its descriptor is retired); `len` <= 1518.
        unsafe {
            let dst = self.dma.host.add(buf);
            for (k, byte) in f.iter().enumerate() { ptr::write_volatile(dst.add(k), *byte); }
            for k in f.len()..len { ptr::write_volatile(dst.add(k), 0); }
        }
        let d = Self::tx_desc(i);
        self.w64(d, self.dma.device + buf as u64);
        self.w32(d + LEN, len as u32 | (u32::from(TX_EOP | TX_IFCS | TX_RS) << 24));
        self.w32(d + STATUS, 0);
        self.tx_prod = self.tx_prod.wrapping_add(1);
        dma_wmb();
        mmio_wmb();
        b.w32(TDT, (self.tx_prod % TX_COUNT) as u32)?;
        Ok(true)
    }

    /// The next good received frame: its length (CRC already stripped,
    /// RCTL.SECRC) and destination, without taking it. Descriptors with
    /// errors, split frames and runts are given back on the way.
    pub fn peek<B: Bus>(&mut self, b: &mut B) -> R<Option<(usize, [u8; 6])>, B::Error> {
        if !self.running { return Ok(None); }
        loop {
            let d = Self::rx_desc(self.rx_next);
            let status = self.r8(d + STATUS);
            if status & DD == 0 { return Ok(None); }
            dma_rmb();
            let len = self.r16(d + LEN) as usize;
            let errors = self.r8(d + RX_ERRORS);
            if errors & RX_FRAME_ERRORS != 0 || status & RX_EOP == 0 || !(MIN_FRAME..=BUF_SIZE).contains(&len) {
                self.rx_errors += 1;
                self.skip(b)?;
                continue;
            }
            let mut dst = [0u8; 6];
            let buf = RX_BUFS + self.rx_next * BUF_SIZE;
            for (k, byte) in dst.iter_mut().enumerate() {
                // SAFETY: inside the received buffer, which the controller
                // handed back (DD set, ordered by dma_rmb).
                *byte = unsafe { ptr::read_volatile(self.dma.host.add(buf + k)) };
            }
            return Ok(Some((len, dst)));
        }
    }

    /// Give the current receive descriptor back without reading it: it
    /// becomes the new tail.
    pub fn skip<B: Bus>(&mut self, b: &mut B) -> R<(), B::Error> {
        let i = self.rx_next;
        self.post_rx(i);
        self.rx_next = (i + 1) % RX_COUNT;
        dma_wmb();
        mmio_wmb();
        Ok(b.w32(RDT, i as u32)?)
    }

    /// Take the next good frame into `out`: Ok(Some(len)), Ok(None) when
    /// nothing is waiting. A frame longer than `out` stays queued
    /// (`Error::Frame` with its length).
    pub fn receive<B: Bus>(&mut self, b: &mut B, out: &mut [u8]) -> R<Option<usize>, B::Error> {
        let Some((len, _)) = self.peek(b)? else { return Ok(None) };
        if len > out.len() { return Err(Error::Frame { len }); }
        let buf = RX_BUFS + self.rx_next * BUF_SIZE;
        for (k, byte) in out[..len].iter_mut().enumerate() {
            // SAFETY: as in `peek`.
            *byte = unsafe { ptr::read_volatile(self.dma.host.add(buf + k)) };
        }
        self.skip(b)?;
        Ok(Some(len))
    }
}

/// The frame `check` sends: broadcast, ethertype 0x88b5 (IEEE local
/// experimental), padded to 60 bytes.
pub fn check_frame(mac: [u8; 6]) -> [u8; 60] {
    let mut f = [0u8; 60];
    f[0..6].copy_from_slice(&[0xff; 6]);
    f[6..12].copy_from_slice(&mac);
    f[12..14].copy_from_slice(&0x88b5u16.to_be_bytes());
    let text = b"stormnic-igb DMA check";
    f[14..14 + text.len()].copy_from_slice(text);
    f
}

/// Send the check frame and wait up to 100 ms for the controller to write
/// DD back: it fetched the descriptor and the buffer and wrote the
/// descriptor, so DMA works both ways. Ok(Some(ms)) when it did.
pub fn check<B: Bus>(b: &mut B, net: &mut Net) -> R<Option<usize>, B::Error> {
    if !net.transmit(b, &check_frame(net.mac))? { return Ok(None); }
    for ms in 0..=100 {
        net.reclaim();
        if net.tx_pending() == 0 { return Ok(Some(ms)); }
        b.delay_us(1000);
    }
    Ok(None)
}
