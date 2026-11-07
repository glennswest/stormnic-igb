//! stormnic-igb: an `EFI_SIMPLE_NETWORK_PROTOCOL` driver for the Intel igb
//! gigabit family (82575, 82576, 82580, I350, I354, I210, I211), in Rust,
//! for x86_64 and aarch64.
//!
//! An EFI boot-service driver (see build.rs). The entry point installs an
//! `EFI_DRIVER_BINDING_PROTOCOL` on the image handle (binding.rs) and returns;
//! the firmware's `ConnectController` then calls Supported/Start for each
//! controller. Start identifies the controller, resets the MAC, brings it up
//! once (flow control off), makes sure the internal PHY is powered and
//! autonegotiating, runs a DMA check when the link is up, quiesces the NIC
//! and puts an `EFI_SIMPLE_NETWORK_PROTOCOL` on a child handle; the user
//! (stormbootx's smoltcp) opens it EXCLUSIVE, and only its Initialize turns
//! RX/TX on.
#![no_main]
#![no_std]

extern crate alloc;

// First: its macros (say!, trace!, fail!, note!) are used by the modules below.
#[macro_use]
mod console;
mod binding;
mod decode;
mod ids;
mod igb;
mod pci_io;
mod snp;
mod trace;

use uefi::prelude::*;

#[entry]
fn main() -> Status {
    if uefi::helpers::init().is_err() {
        return Status::LOAD_ERROR;
    }
    console::init();
    log::set_max_level(if console::verbose() { log::LevelFilter::Info } else { log::LevelFilter::Warn });
    let version = env!("CARGO_PKG_VERSION");
    match binding::install() {
        Ok(()) => {
            trace!(
                "stormnic-igb {version}: driver binding installed ({} Intel igb device IDs), flow control off",
                ids::SUPPORTED.len()
            );
            Status::SUCCESS
        }
        Err(e) => {
            say!("stormnic-igb {version}: driver binding not installed: {:?}", e.status());
            e.status()
        }
    }
}
