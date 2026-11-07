//! The SNP state machine and data path against the simulated controller in
//! `sim.rs`: the calls stormbootx's smoltcp (or a firmware MNP) makes, the
//! failure statuses from the UEFI specification, and the rule that the NIC
//! is quiesced in every state but Initialized.
#[path = "../src/igb.rs"]
#[allow(dead_code)]
mod igb;
#[allow(dead_code)]
mod sim;
use igb::ring::BUF_SIZE;
use igb::snp::{self, Fail, Received, Snp, State};
use igb::Link;
use sim::*;

const OTHER: [u8; 6] = [0x02, 9, 9, 9, 9, 9];
const MCAST: [u8; 6] = [0x01, 0x00, 0x5e, 0x00, 0x00, 0xfb];
type R<T> = Result<T, Fail<()>>;

fn snp() -> (Dev, Snp) {
    let d = Dev::new();
    let net = d.net();
    (d, Snp::new(net, Link { up: true, mbps: 1000, full_duplex: true }))
}

/// Start, Initialize and the filters smoltcp asks for: unicast + broadcast.
fn up() -> (Dev, Snp) {
    let (mut d, mut s) = snp();
    s.start::<()>().unwrap();
    s.initialize(&mut d).unwrap();
    s.receive_filters(&mut d, snp::UNICAST | snp::BROADCAST, 0, false, &[]).unwrap();
    (d, s)
}

fn rx(d: &mut Dev, s: &mut Snp) -> R<Received> {
    let mut buf = [0u8; BUF_SIZE];
    s.receive(d, &mut buf)
}

#[test]
fn calls_before_start_and_initialize_fail_as_the_spec_says() {
    let (mut d, mut s) = snp();
    assert_eq!(s.initialize(&mut d), Err(Fail::NotStarted));
    assert_eq!(rx(&mut d, &mut s), Err(Fail::NotStarted));
    s.start::<()>().unwrap();
    assert_eq!(s.start::<()>(), Err(Fail::AlreadyStarted));
    assert_eq!(rx(&mut d, &mut s), Err(Fail::NotInitialized));
    assert_eq!(s.get_status(&mut d, false), Err(Fail::NotInitialized));
    assert!(!d.rx_enabled() && !d.tx_enabled(), "started is not running");
}

#[test]
fn initialize_runs_the_nic_with_the_link_back_and_shutdown_quiesces_it() {
    let (mut d, mut s) = up();
    assert_eq!(s.state, State::Initialized);
    assert!(d.rx_enabled() && d.tx_enabled());
    assert!(s.media, "Initialize waited for the MAC's side of the link");
    assert_eq!(d.an_restarts, 0, "the PHY is never renegotiated by Initialize");
    assert_eq!(d.get(igb::CTRL) & (igb::CTRL_TFCE | igb::CTRL_RFCE), 0);
    s.shutdown(&mut d).unwrap();
    assert_eq!(s.state, State::Started);
    assert!(!d.rx_enabled() && !d.tx_enabled());
    s.initialize(&mut d).unwrap();
    assert!(d.rx_enabled());
    s.stop(&mut d).unwrap();
    assert_eq!(s.state, State::Stopped);
    assert!(!d.rx_enabled() && !d.tx_enabled());
}

#[test]
fn no_cable_initializes_without_waiting_and_get_status_sees_it_later() {
    let d = Dev::new();
    let net = d.net();
    let mut s = Snp::new(net, Link::default());
    let mut d = d;
    d.cable = false;
    s.start::<()>().unwrap();
    let t = d.now;
    s.initialize(&mut d).unwrap();
    assert!(!s.media);
    assert!(d.now - t < 200_000, "waited {} us for a link that was down", d.now - t);
    d.cable = true;
    d.now += RELINK_US;
    s.get_status(&mut d, false).unwrap();
    assert!(s.media);
}

#[test]
fn halt_quiesces_in_every_state() {
    for state in [State::Stopped, State::Started, State::Initialized] {
        let (mut d, mut s) = snp();
        if state != State::Stopped { s.start::<()>().unwrap(); }
        if state == State::Initialized { s.initialize(&mut d).unwrap(); }
        let resets = d.resets;
        s.halt(&mut d).unwrap();
        assert_eq!(d.resets, resets + 1, "{state:?}");
        assert!(!d.rx_enabled() && !d.tx_enabled());
        assert_eq!(s.state, State::Stopped);
    }
}

#[test]
fn a_failed_initialize_leaves_it_started_and_quiesced() {
    let (mut d, mut s) = snp();
    s.start::<()>().unwrap();
    d.queue_stuck = true;
    assert!(matches!(s.initialize(&mut d), Err(Fail::Device(_))));
    assert_eq!(s.state, State::Started);
    assert!(!d.rx_enabled() && !d.tx_enabled());
}

#[test]
fn frames_are_filtered_by_the_setting() {
    let (mut d, mut s) = up();
    d.deliver(&frame_to(OTHER, 1));
    d.deliver(&frame_to(MCAST, 2));
    d.deliver(&frame_to([0xff; 6], 3));
    d.deliver(&frame_to(MAC, 4));
    let r = rx(&mut d, &mut s).unwrap();
    assert_eq!((r.destination, r.len), ([0xff; 6], 60));
    assert_eq!(rx(&mut d, &mut s).unwrap().destination, MAC);
    assert_eq!(rx(&mut d, &mut s), Err(Fail::NotReady));
    s.receive_filters(&mut d, snp::MULTICAST, 0, false, &[MCAST]).unwrap();
    d.deliver(&frame_to(MCAST, 5));
    assert_eq!(rx(&mut d, &mut s).unwrap().destination, MCAST);
    assert_eq!(s.receive_filters(&mut d, 0, 0, false, &[OTHER]), Err(Fail::InvalidParameter));
}

#[test]
fn promiscuous_reaches_the_controller() {
    let (mut d, mut s) = up();
    s.receive_filters(&mut d, snp::PROMISCUOUS, 0, false, &[]).unwrap();
    assert_eq!(d.get(igb::RCTL) & igb::RCTL_UPE, igb::RCTL_UPE);
    d.deliver(&frame_to(OTHER, 1));
    assert_eq!(rx(&mut d, &mut s).unwrap().destination, OTHER);
    s.receive_filters(&mut d, 0, snp::PROMISCUOUS, false, &[]).unwrap();
    assert_eq!(d.get(igb::RCTL) & igb::RCTL_UPE, 0);
    // A Reset keeps the setting.
    s.receive_filters(&mut d, snp::PROMISCUOUS, 0, false, &[]).unwrap();
    s.reset(&mut d).unwrap();
    assert_eq!(d.get(igb::RCTL) & igb::RCTL_UPE, igb::RCTL_UPE);
}

#[test]
fn transmit_fills_the_header_and_get_status_hands_buffers_back_in_order() {
    let (mut d, mut s) = up();
    let mut f = vec![0u8; 60];
    s.transmit(&mut d, 14, &mut f, None, Some([0xff; 6]), Some(0x0806), 0x1000).unwrap();
    assert_eq!(&d.wire[0][0..14], &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, MAC[0], MAC[1], MAC[2], MAC[3], MAC[4], MAC[5], 0x08, 0x06]);
    let mut g = frame_to(OTHER, 2);
    s.transmit(&mut d, 0, &mut g, None, None, None, 0x2000).unwrap();
    let (bits, t) = s.get_status(&mut d, true).unwrap();
    assert_eq!((bits & snp::TRANSMIT_INTERRUPT, t), (snp::TRANSMIT_INTERRUPT, Some(0x1000)));
    assert_eq!(s.get_status(&mut d, true).unwrap().1, Some(0x2000));
    assert_eq!(s.get_status(&mut d, true).unwrap(), (0, None));
    assert_eq!(s.transmit(&mut d, 14, &mut f, None, None, Some(1), 1), Err(Fail::InvalidParameter));
    assert_eq!(s.transmit(&mut d, 0, &mut [0u8; 10], None, None, None, 1), Err(Fail::BufferTooSmall(14)));
}

#[test]
fn a_frame_bigger_than_the_buffer_reports_its_length_and_stays() {
    let (mut d, mut s) = up();
    d.deliver(&frame_to(MAC, 1));
    assert_eq!(s.receive(&mut d, &mut [0u8; 30]), Err(Fail::BufferTooSmall(60)));
    assert_eq!(rx(&mut d, &mut s).unwrap().len, 60);
}

#[test]
fn station_address_takes_only_the_nvm_s() {
    let (_d, mut s) = up();
    assert_eq!(s.station_address::<()>(false, Some(MAC)), Ok(()));
    assert_eq!(s.station_address::<()>(false, Some(OTHER)), Err(Fail::Unsupported));
    assert_eq!(s.station_address::<()>(true, None), Ok(()));
}

#[test]
fn wait_for_packet_sees_only_accepted_frames() {
    let (mut d, mut s) = up();
    d.deliver(&frame_to(OTHER, 1));
    assert!(!s.frame_waiting(&mut d));
    d.deliver(&frame_to(MAC, 2));
    assert!(s.frame_waiting(&mut d));
}

#[test]
fn a_long_exchange_never_runs_out_of_buffers() {
    let (mut d, mut s) = up();
    for i in 0..500u32 {
        d.deliver(&frame_to(MAC, i as u8));
        assert_eq!(rx(&mut d, &mut s).unwrap().len, 60, "frame {i}");
        let mut f = frame_to(OTHER, i as u8);
        s.transmit(&mut d, 0, &mut f, None, None, None, i as usize).unwrap();
        assert_eq!(s.get_status(&mut d, true).unwrap().1, Some(i as usize));
    }
    assert_eq!(d.dropped, 0);
    assert_eq!(d.wire.len(), 500);
}

#[test]
fn mcast_ip_to_mac_maps_both_families() {
    let mut v4 = [0u8; 16];
    v4[..4].copy_from_slice(&[224, 0x80, 0, 251]);
    assert_eq!(snp::mcast_ip_to_mac(false, &v4), Some([0x01, 0x00, 0x5e, 0x00, 0x00, 251]));
    let mut v6 = [0u8; 16];
    v6[0] = 0xff;
    v6[12..].copy_from_slice(&[1, 2, 3, 4]);
    assert_eq!(snp::mcast_ip_to_mac(true, &v6), Some([0x33, 0x33, 1, 2, 3, 4]));
    assert_eq!(snp::mcast_ip_to_mac(false, &[10; 16]), None);
}
