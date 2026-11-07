//! The MAC/PHY sequences and the rings against the simulated controller in
//! `sim.rs`.
#[path = "../src/igb.rs"]
#[allow(dead_code)]
mod igb;
#[allow(dead_code)]
mod sim;
use igb::ring::{self, Net, RX_COUNT, TX_COUNT};
use igb::{Error, Link, Media, Phy, PhySkip};
use sim::*;

#[test]
fn identify_reads_status_media_address_and_link_without_writing() {
    let mut d = Dev::new();
    d.function = 2;
    let id = igb::identify(&mut d).unwrap();
    assert_eq!((id.func, id.media, id.mac), (2, Media::Copper, Some(MAC)));
    assert_eq!(id.link, Link { up: true, mbps: 1000, full_duplex: true });
    assert!(d.ops.is_empty(), "identify writes nothing");
}

#[test]
fn a_dead_function_is_removed_and_the_media_comes_from_ctrl_ext() {
    let mut d = Dev::new();
    d.regs.insert(igb::CTRL_EXT, 3 << igb::CTRL_EXT_LINK_MODE_SHIFT);
    assert_eq!(igb::identify(&mut d).unwrap().media, Media::Serdes);
    d.regs.insert(igb::CTRL_EXT, 2 << igb::CTRL_EXT_LINK_MODE_SHIFT);
    assert_eq!(igb::identify(&mut d).unwrap().media, Media::Sgmii);
    struct Gone;
    impl igb::Bus for Gone {
        type Error = ();
        fn r32(&mut self, _: u32) -> Result<u32, ()> { Ok(u32::MAX) }
        fn w32(&mut self, _: u32, _: u32) -> Result<(), ()> { Ok(()) }
        fn delay_us(&mut self, _: usize) {}
    }
    assert_eq!(igb::identify(&mut Gone), Err(Error::Removed));
}

#[test]
fn an_address_without_av_or_not_unicast_is_none() {
    let mut d = Dev::new();
    let rah = d.get(igb::RAH0);
    d.regs.insert(igb::RAH0, rah & !igb::RAH_AV);
    assert_eq!(igb::read_mac(&mut d), Ok(None));
    d.regs.insert(igb::RAH0, rah);
    d.regs.insert(igb::RAL0, 0x0000_0001); // 01:00:00:00:…: multicast
    assert_eq!(igb::read_mac(&mut d), Ok(None));
}

#[test]
fn status_decodes_speed_and_duplex() {
    let l = Link::from_status(igb::STATUS_LU | igb::STATUS_FD | 1 << igb::STATUS_SPEED_SHIFT);
    assert_eq!(format!("{l}"), "up 100 Mb/s full duplex");
    let l = Link::from_status(igb::STATUS_LU | 3 << igb::STATUS_SPEED_SHIFT);
    assert_eq!(format!("{l}"), "up 1000 Mb/s half duplex");
    assert_eq!(format!("{}", Link::from_status(igb::STATUS_FD)), "down");
}

#[test]
fn reset_drains_bus_mastering_first_and_reloads_the_nvm_address() {
    let mut d = Dev::new();
    d.regs.insert(igb::RAL0, 0x0403_0202); // a previous driver's address
    let r = igb::reset(&mut d).unwrap();
    assert!(r.master_drained && r.nvm_loaded);
    assert_eq!(d.resets, 1);
    assert_eq!(igb::read_mac(&mut d), Ok(Some(MAC)));
    assert_eq!(d.get(igb::CTRL) & igb::CTRL_RST, 0);
    assert_eq!(d.writes(igb::IMC), vec![u32::MAX]);
    // GIO master disable went in before RST (the sim asserts it).
    let ctrl = d.writes(igb::CTRL);
    assert!(ctrl[0] & igb::CTRL_GIO_MASTER_DISABLE != 0 && ctrl[0] & igb::CTRL_RST == 0);
}

#[test]
fn a_stuck_bus_master_is_reported_not_fatal_and_a_stuck_reset_is() {
    let mut d = Dev::new();
    d.master_stuck = true;
    assert!(!igb::reset(&mut d).unwrap().master_drained);
    let mut d = Dev::new();
    d.reset_stuck = true;
    assert!(matches!(igb::reset(&mut d), Err(Error::ResetTimeout { .. })));
}

// Flow control off.

#[test]
fn start_turns_flow_control_off_and_drops_when_the_ring_is_full() {
    let (d, net) = started();
    assert!(net.running && d.rx_enabled() && d.tx_enabled());
    let ctrl = d.get(igb::CTRL);
    assert_eq!(ctrl & (igb::CTRL_TFCE | igb::CTRL_RFCE), 0, "the firmware's flow control was left on");
    assert!(ctrl & igb::CTRL_SLU != 0);
    assert_eq!((d.get(igb::FCRTL), d.get(igb::FCRTH)), (0, 0));
    let srrctl = d.get(igb::SRRCTL);
    assert!(srrctl & igb::SRRCTL_DROP_EN != 0);
    assert_eq!(srrctl & 0x7f, 2, "2 KB buffers");
    assert_eq!(srrctl & (7 << 25), 1 << 25, "advanced one-buffer receive descriptors");
    assert!(d.get(igb::CTRL_EXT) & igb::CTRL_EXT_DRV_LOAD != 0);
}

#[test]
fn start_programs_queue_0_and_hands_over_all_but_one_buffer() {
    let (d, net) = started();
    let rings = net.rings();
    assert_eq!(d.rx_ring(), rings.rx);
    assert_eq!(d.tx_ring(), rings.tx);
    assert_eq!(d.get(igb::RDLEN), (RX_COUNT * 16) as u32);
    assert_eq!(d.get(igb::TDLEN), (TX_COUNT * 16) as u32);
    assert_eq!(rings.rx % 128, 0);
    assert_eq!(rings.tx % 128, 0);
    assert_eq!(d.get(igb::RDT), (RX_COUNT - 1) as u32);
    assert_eq!(d.rx_free(), (RX_COUNT - 1) as u32);
    let rctl = d.get(igb::RCTL);
    assert_eq!(rctl & (igb::RCTL_EN | igb::RCTL_BAM | igb::RCTL_MPE | igb::RCTL_SECRC | igb::RCTL_UPE),
        igb::RCTL_EN | igb::RCTL_BAM | igb::RCTL_MPE | igb::RCTL_SECRC);
    assert_eq!(d.get(igb::RAH0) & igb::RAH_AV, igb::RAH_AV);
    // RDT is moved only after the queue is enabled; RCTL.EN is last.
    let i_rxdctl = d.ops.iter().rposition(|o| o.0 == igb::RXDCTL).unwrap();
    let i_rdt = d.ops.iter().rposition(|o| o.0 == igb::RDT).unwrap();
    let i_rctl = d.ops.iter().rposition(|o| o.0 == igb::RCTL).unwrap();
    assert!(i_rxdctl < i_rdt && i_rdt < i_rctl);
}

#[test]
fn a_queue_that_will_not_enable_fails_start_and_leaves_it_quiesced() {
    let mut d = Dev::new();
    d.queue_stuck = true;
    let mut net = d.net();
    assert!(matches!(net.start(&mut d, false), Err(Error::QueueTimeout { tx: false, .. })));
    assert!(!net.running && !d.rx_enabled() && !d.tx_enabled());
    assert_eq!(d.get(igb::TCTL) & igb::TCTL_EN, 0);
}

#[test]
fn stop_quiesces_the_controller() {
    let (mut d, mut net) = started();
    let resets = d.resets;
    net.stop(&mut d).unwrap();
    assert_eq!(d.resets, resets + 1);
    assert!(!d.rx_enabled() && !d.tx_enabled());
    assert_eq!(d.get(igb::RCTL) & igb::RCTL_EN, 0);
    assert_eq!(d.get(igb::CTRL_EXT) & igb::CTRL_EXT_DRV_LOAD, 0);
}

// The PHY.

#[test]
fn a_powered_autonegotiating_phy_is_not_touched() {
    let mut d = Dev::new();
    let id = igb::identify(&mut d).unwrap();
    assert_eq!(igb::phy_up(&mut d, &id).unwrap(), Phy::Fine { bmcr: BMCR_DEFAULT });
    assert_eq!(d.an_restarts, 0);
    assert_eq!(d.phy_unlocked_accesses, 0, "PHY read without the semaphore");
    assert_eq!(d.get(igb::SW_FW_SYNC), 0, "the PHY bit was not given back");
    assert_eq!(d.get(igb::SWSM), 0, "SWSM was not given back");
}

#[test]
fn a_powered_down_phy_is_powered_up_and_renegotiates() {
    let mut d = Dev::new();
    d.phy.insert(igb::MII_BMCR, igb::BMCR_PDOWN | igb::BMCR_ISOLATE | 0x0140);
    let id = igb::identify(&mut d).unwrap();
    assert!(!id.link.up);
    let before = igb::BMCR_PDOWN | igb::BMCR_ISOLATE | 0x0140;
    let after = igb::BMCR_ANENABLE | 0x0140;
    assert_eq!(igb::phy_up(&mut d, &id).unwrap(), Phy::Restarted { before, after });
    assert_eq!(d.phy[&igb::MII_BMCR], after);
    assert_eq!(d.an_restarts, 1);
    assert_eq!(d.phy_unlocked_accesses, 0);
    assert_eq!(d.get(igb::SW_FW_SYNC), 0);
    assert!(!igb::wait_link(&mut d, 1000).unwrap().up);
    assert!(igb::wait_link(&mut d, 5000).unwrap().up);
}

#[test]
fn the_phy_bit_follows_the_function() {
    let mut d = Dev::new();
    d.function = 3;
    let id = igb::identify(&mut d).unwrap();
    igb::phy_up(&mut d, &id).unwrap();
    assert_eq!(d.phy_unlocked_accesses, 0);
    assert!(d.writes(igb::SW_FW_SYNC).contains(&0x40), "port 3's bit is 0x40");
}

#[test]
fn a_phy_the_firmware_holds_is_left_alone() {
    let mut d = Dev::new();
    d.fw_holds_phy = true;
    d.phy.insert(igb::MII_BMCR, igb::BMCR_PDOWN);
    let id = igb::identify(&mut d).unwrap();
    assert!(matches!(igb::phy_up(&mut d, &id).unwrap(), Phy::Skipped(PhySkip::Busy { .. })));
    assert_eq!(d.phy_locked_accesses + d.phy_unlocked_accesses, 0);
    assert!(d.now < 2_000_000, "gave up in about 1 s ({} us)", d.now);
    assert_eq!(d.get(igb::SWSM), 0, "SWSM left taken");
    assert_eq!(d.get(igb::SW_FW_SYNC) & 0xffff, 0, "a software bit left set");
}

#[test]
fn no_internal_phy_off_copper() {
    let mut d = Dev::new();
    d.regs.insert(igb::CTRL_EXT, 3 << igb::CTRL_EXT_LINK_MODE_SHIFT);
    let id = igb::identify(&mut d).unwrap();
    assert_eq!(igb::phy_up(&mut d, &id).unwrap(), Phy::Skipped(PhySkip::NotCopper(Media::Serdes)));
    assert_eq!(d.phy_locked_accesses + d.phy_unlocked_accesses, 0);
}

#[test]
fn a_stuck_mdic_is_an_error_and_the_semaphore_is_still_given_back() {
    let mut d = Dev::new();
    d.mdic_stuck = true;
    let id = igb::identify(&mut d).unwrap();
    assert!(matches!(igb::phy_up(&mut d, &id), Err(Error::Mdic { reg: 0, .. })));
    assert_eq!(d.get(igb::SW_FW_SYNC), 0);
    assert_eq!(d.get(igb::SWSM), 0);
}

// The rings.

#[test]
fn frames_go_out_padded_with_a_status_report() {
    let (mut d, mut net) = started();
    let f = frame_to([0xff; 6], 1);
    assert!(net.transmit(&mut d, &f[..42]).unwrap());
    assert_eq!(d.wire.len(), 1);
    assert_eq!(d.wire[0].len(), 60);
    assert_eq!(&d.wire[0][..42], &f[..42]);
    assert!(d.wire[0][42..].iter().all(|b| *b == 0));
    net.reclaim();
    assert_eq!(net.tx_pending(), 0);
    assert!(matches!(net.transmit(&mut d, &[0u8; 10]), Err(Error::Frame { len: 10 })));
    assert!(matches!(net.transmit(&mut d, &[0u8; 1519]), Err(Error::Frame { len: 1519 })));
}

#[test]
fn transmit_fills_at_most_count_minus_one_then_reports_full() {
    let (mut d, mut net) = started();
    d.regs.insert(igb::TCTL, 0); // transmitter stalled: nothing completes
    let f = frame_to([0xff; 6], 0);
    for _ in 0..TX_COUNT - 1 { assert!(net.transmit(&mut d, &f).unwrap()); }
    assert!(!net.transmit(&mut d, &f).unwrap(), "TDT = TDH would mean empty");
    assert_eq!(net.tx_pending(), TX_COUNT - 1);
}

#[test]
fn the_transmit_ring_wraps() {
    let (mut d, mut net) = started();
    for i in 0..3 * TX_COUNT {
        assert!(net.transmit(&mut d, &frame_to([0xff; 6], i as u8)).unwrap(), "frame {i}");
    }
    assert_eq!(d.wire.len(), 3 * TX_COUNT);
    assert_eq!(d.wire[3 * TX_COUNT - 1][14], (3 * TX_COUNT - 1) as u8);
}

#[test]
fn received_frames_come_in_order_and_the_ring_wraps() {
    let (mut d, mut net) = started();
    let mut buf = [0u8; ring::BUF_SIZE];
    for i in 0..3 * RX_COUNT {
        d.deliver(&frame_to(MAC, i as u8));
        assert_eq!(net.receive(&mut d, &mut buf).unwrap(), Some(60), "frame {i}");
        assert_eq!(buf[14], i as u8);
    }
    assert_eq!(net.receive(&mut d, &mut buf).unwrap(), None);
    assert_eq!(d.dropped, 0);
    assert_eq!(d.rx_free(), (RX_COUNT - 1) as u32, "every buffer went back");
}

#[test]
fn a_full_ring_drops_and_recovers() {
    let (mut d, mut net) = started();
    for i in 0..RX_COUNT + 5 { d.deliver(&frame_to(MAC, i as u8)); }
    assert_eq!(d.dropped, 6, "31 buffers posted, the rest dropped");
    let mut buf = [0u8; ring::BUF_SIZE];
    let mut n = 0;
    while net.receive(&mut d, &mut buf).unwrap().is_some() { n += 1; }
    assert_eq!(n, RX_COUNT - 1);
    d.deliver(&frame_to(MAC, 0xaa));
    assert_eq!(net.receive(&mut d, &mut buf).unwrap(), Some(60));
    assert_eq!(buf[14], 0xaa);
}

#[test]
fn bad_descriptors_are_skipped_and_counted() {
    let (mut d, mut net) = started();
    d.deliver_raw(ring::RX_DD | ring::RX_EOP | 1 << 24, 60); // CRC error
    d.deliver_raw(ring::RX_DD, 60); // not the end of the frame
    d.deliver_raw(ring::RX_DD | ring::RX_EOP, 6); // runt
    d.deliver(&frame_to(MAC, 7));
    let mut buf = [0u8; ring::BUF_SIZE];
    assert_eq!(net.receive(&mut d, &mut buf).unwrap(), Some(60));
    assert_eq!(buf[14], 7);
    assert_eq!(net.rx_errors, 3);
}

#[test]
fn a_checksum_error_alone_is_still_a_good_frame() {
    let (mut d, mut net) = started();
    d.deliver(&frame_to(MAC, 1));
    // Mark the descriptor just written with the L4 checksum error bit (29).
    let desc = (net.rings().rx - DEVICE_BASE) as usize;
    // SAFETY: the first receive descriptor's status/error byte 3, inside the region.
    unsafe { *d.dma().host.add(desc + 11) = 0x20; }
    let mut buf = [0u8; ring::BUF_SIZE];
    assert_eq!(net.receive(&mut d, &mut buf).unwrap(), Some(60));
}

#[test]
fn a_frame_bigger_than_the_buffer_stays_queued() {
    let (mut d, mut net) = started();
    d.deliver(&frame_to(MAC, 1));
    assert!(matches!(net.receive(&mut d, &mut [0u8; 20]), Err(Error::Frame { len: 60 })));
    assert_eq!(net.receive(&mut d, &mut [0u8; 64]).unwrap(), Some(60));
}

#[test]
fn the_dma_check_sees_the_descriptor_back() {
    let (mut d, mut net) = started();
    assert_eq!(ring::check(&mut d, &mut net).unwrap(), Some(0));
    assert_eq!(&d.wire[0][0..6], &[0xff; 6]);
    assert_eq!(&d.wire[0][12..14], &[0x88, 0xb5]);
    let (mut d, mut net) = started();
    d.regs.insert(igb::TCTL, 0);
    assert_eq!(ring::check(&mut d, &mut net).unwrap(), None);
}

#[test]
fn promiscuous_reaches_rctl_only_while_running() {
    let (mut d, net) = started();
    net.set_promiscuous(&mut d, true).unwrap();
    assert!(d.get(igb::RCTL) & igb::RCTL_UPE != 0 && d.get(igb::RCTL) & igb::RCTL_EN != 0);
    net.set_promiscuous(&mut d, false).unwrap();
    assert_eq!(d.get(igb::RCTL) & igb::RCTL_UPE, 0);
    let d2 = Dev::new();
    let net2: Net = d2.net();
    assert!(!net2.running);
}

// Relink after a MAC reset.

#[test]
fn a_real_controller_relinks_without_renegotiating() {
    let mut d = Dev::new();
    let mut net = d.net();
    net.start(&mut d, false).unwrap();
    let r = igb::relink(&mut d, 0, Media::Copper, 2000).unwrap();
    assert!(r.link.up && !r.restarted);
    assert_eq!(d.an_restarts, 0);
}

#[test]
fn qemu_s_model_gets_its_link_back_by_renegotiating() {
    let mut d = Dev::new();
    d.qemu_relink = true;
    let mut net = d.net();
    net.start(&mut d, false).unwrap();
    assert!(!igb::link(&mut d).unwrap().up, "the reset dropped STATUS.LU");
    let r = igb::relink(&mut d, 0, Media::Copper, 0).unwrap();
    assert!(r.restarted && r.link.up, "{r:?}");
    assert_eq!(r.bmsr.map(|b| b & igb::BMSR_LINK), Some(igb::BMSR_LINK));
    assert_eq!(d.an_restarts, 1);
    assert_eq!(d.phy_unlocked_accesses, 0);
    assert_eq!(d.get(igb::SW_FW_SYNC), 0);
}

#[test]
fn no_cable_is_not_renegotiated() {
    let mut d = Dev::new();
    d.qemu_relink = true;
    d.cable = false;
    let mut net = d.net();
    net.start(&mut d, false).unwrap();
    let t = d.now;
    let r = igb::relink(&mut d, 0, Media::Copper, 0).unwrap();
    assert!(!r.restarted && !r.link.up);
    assert_eq!(r.bmsr.map(|b| b & igb::BMSR_LINK), Some(0));
    assert!(d.now - t < 100_000, "waited {} us", d.now - t);
}
