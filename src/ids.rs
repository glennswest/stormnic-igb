//! The PCI functions this driver binds, checked against pci.ids
//! (pci-ids.ucw.cz, version 2026.11.06).
//!
//! Every igb physical function with a MAC the driver knows. Not here: the
//! virtual functions (82576 VF 10ca, I350 VF 1520/152f: no NVM, no PHY,
//! they need a PF driver), the I210 "unprogrammed" ID (1531: a blank NVM, no
//! station address), and the e1000e (82574, PCH) and igc (I225/I226) parts,
//! which have other register sets.
//!
//! 1534 and 1535 (I210 OEM and industrial copper) are not in pci.ids; they
//! are I210 copper IDs in the device ID list of Intel's shared code
//! (FreeBSD `sys/dev/e1000/e1000_hw.h`, BSD-3).

use crate::igb::Mac;

/// PCI class: network controller.
pub const CLASS_NETWORK: u8 = 0x02;

pub struct Nic {
    pub device: u16,
    pub mac: Mac,
    pub name: &'static str,
}

const fn n(device: u16, mac: Mac, name: &'static str) -> Nic { Nic { device, mac, name } }

pub const SUPPORTED: &[Nic] = &[
    n(0x10a7, Mac::M82575, "82575EB"),
    n(0x10a9, Mac::M82575, "82575EB backplane"),
    n(0x10d6, Mac::M82575, "82575GB"),
    n(0x10e2, Mac::M82575, "82575GB"),
    n(0x10c9, Mac::M82576, "82576"),
    n(0x10e6, Mac::M82576, "82576 fiber"),
    n(0x10e7, Mac::M82576, "82576 SerDes"),
    n(0x10e8, Mac::M82576, "82576 quad copper"),
    n(0x150a, Mac::M82576, "82576NS"),
    n(0x150d, Mac::M82576, "82576 backplane"),
    n(0x1518, Mac::M82576, "82576NS SerDes"),
    n(0x1526, Mac::M82576, "82576 quad copper ET2"),
    n(0x1509, Mac::M82580, "82580"),
    n(0x150e, Mac::M82580, "82580"),
    n(0x150f, Mac::M82580, "82580 fiber"),
    n(0x1510, Mac::M82580, "82580 backplane"),
    n(0x1511, Mac::M82580, "82580 SFP"),
    n(0x1516, Mac::M82580, "82580"),
    n(0x1527, Mac::M82580, "82580 fiber"),
    n(0x1521, Mac::I350, "I350"),
    n(0x1522, Mac::I350, "I350 fiber"),
    n(0x1523, Mac::I350, "I350 backplane"),
    n(0x1524, Mac::I350, "I350 SGMII"),
    n(0x1f40, Mac::I354, "I354 1.0 GbE backplane"),
    n(0x1f41, Mac::I354, "I354"),
    n(0x1f45, Mac::I354, "I354 2.5 GbE backplane"),
    n(0x1533, Mac::I210, "I210"),
    n(0x1534, Mac::I210, "I210 OEM"),
    n(0x1535, Mac::I210, "I210 IT"),
    n(0x1536, Mac::I210, "I210 fiber"),
    n(0x1537, Mac::I210, "I210 backplane"),
    n(0x1538, Mac::I210, "I210 SGMII"),
    n(0x157b, Mac::I210, "I210 (flashless)"),
    n(0x157c, Mac::I210, "I210 backplane (flashless)"),
    n(0x15f6, Mac::I210, "I210 SGMII (flashless)"),
    n(0x1539, Mac::I211, "I211"),
];

pub fn lookup(vendor: u16, device: u16) -> Option<&'static Nic> {
    if vendor != crate::igb::VENDOR {
        return None;
    }
    SUPPORTED.iter().find(|n| n.device == device)
}
