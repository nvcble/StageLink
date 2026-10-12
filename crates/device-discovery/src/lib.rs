//! Network adapter enumeration and service discovery.
//!
//! ## Phase status
//!
//! Adapter enumeration and the network warnings in [`adapters`] are complete —
//! they are what the Network page and the setup wizard need in order to tell an
//! operator "you have selected your Wi-Fi card, but the access point is on the
//! Ethernet one".
//!
//! The mDNS/DNS-SD advertiser in [`advertise`] defines the service type and
//! TXT-record contract that StageLink Mobile will browse for. Publishing is
//! Phase 3/5 work, and is gated behind an explicit call so a development build
//! never announces itself on a venue network by accident.
//!
//! A standing design note: discovery is a *convenience*, never an authority. A
//! receiver that finds the desktop by mDNS still has to pair and authenticate.
//! Nothing is trusted because it answered a multicast query.

pub mod adapters;
pub mod advertise;

pub use adapters::{
    AdapterKind, IpAddressInfo, IpFamily, NetworkAdapter, NetworkWarning, NetworkWarningCode, Severity,
    evaluate_warnings,
};
pub use advertise::{SERVICE_TYPE, ServiceAdvertisement, TXT_KEY_CONTROL_PORT};
