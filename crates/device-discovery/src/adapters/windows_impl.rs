//! `GetAdaptersAddresses`-backed adapter enumeration.

use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
    GAA_FLAG_SKIP_MULTICAST, GET_ADAPTERS_ADDRESSES_FLAGS, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};

use super::{AdapterError, AdapterKind, IpAddressInfo, IpFamily, NetworkAdapter};

/// `ERROR_BUFFER_OVERFLOW`: the buffer we offered was too small.
const ERROR_BUFFER_OVERFLOW: u32 = 111;
const NO_ERROR: u32 = 0;

/// `IF_TYPE_*` values from the IANA interface-type registry.
const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_TYPE_TUNNEL: u32 = 131;
const IF_TYPE_PPP: u32 = 23;

pub fn enumerate() -> Result<Vec<NetworkAdapter>, AdapterError> {
    // Ask for the required size, then allocate. The adapter set can change
    // between the two calls, so retry a bounded number of times rather than
    // looping forever.
    let flags = GET_ADAPTERS_ADDRESSES_FLAGS(
        GAA_FLAG_SKIP_ANYCAST.0
            | GAA_FLAG_SKIP_MULTICAST.0
            | GAA_FLAG_SKIP_DNS_SERVER.0
            | GAA_FLAG_INCLUDE_GATEWAYS.0,
    );

    let mut size = 16_384_u32;
    let mut buffer = vec![0_u8; size as usize];

    for _ in 0..4 {
        // SAFETY: `buffer` is `size` bytes and correctly aligned for
        // IP_ADAPTER_ADDRESSES_LH because Vec<u8> from this allocator is
        // 8-byte aligned in practice; we read every field with
        // `read_unaligned` regardless, so alignment is not relied upon.
        let result = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                flags,
                None,
                Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &raw mut size,
            )
        };
        match result {
            NO_ERROR => {
                // SAFETY: on success the buffer holds a linked list of adapters.
                return Ok(unsafe { collect(buffer.as_ptr().cast()) });
            }
            ERROR_BUFFER_OVERFLOW => {
                buffer = vec![0_u8; size as usize];
            }
            other => return Err(AdapterError::SystemCall(other)),
        }
    }

    Err(AdapterError::SystemCall(ERROR_BUFFER_OVERFLOW))
}

/// Walks the adapter linked list.
///
/// # Safety
/// `head` must point at a valid `IP_ADAPTER_ADDRESSES_LH` list as produced by
/// `GetAdaptersAddresses`, with the backing buffer still alive.
unsafe fn collect(head: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<NetworkAdapter> {
    let mut adapters = Vec::new();
    let mut current = head;

    while !current.is_null() {
        // Read by value: the struct is packed on some targets and the buffer
        // has no alignment guarantee.
        let entry = unsafe { core::ptr::read_unaligned(current) };

        let id = unsafe { entry.AdapterName.to_string() }.unwrap_or_default();
        let name = unsafe { entry.FriendlyName.to_string() }.unwrap_or_default();
        let description = unsafe { entry.Description.to_string() }.unwrap_or_default();

        let mac_address = if entry.PhysicalAddressLength > 0 {
            let length = (entry.PhysicalAddressLength as usize).min(entry.PhysicalAddress.len());
            Some(
                entry.PhysicalAddress[..length]
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(":"),
            )
        } else {
            None
        };

        let kind = match entry.IfType {
            IF_TYPE_ETHERNET_CSMACD => AdapterKind::Ethernet,
            IF_TYPE_IEEE80211 => AdapterKind::Wifi,
            IF_TYPE_SOFTWARE_LOOPBACK => AdapterKind::Loopback,
            IF_TYPE_TUNNEL | IF_TYPE_PPP => AdapterKind::Virtual,
            _ => AdapterKind::Other,
        };

        // TransmitLinkSpeed is bits/sec; u64::MAX means "unknown".
        let speed_mbps = if entry.TransmitLinkSpeed == u64::MAX || entry.TransmitLinkSpeed == 0 {
            None
        } else {
            u32::try_from(entry.TransmitLinkSpeed / 1_000_000).ok()
        };

        // GAA_FLAG_INCLUDE_GATEWAYS populates FirstGatewayAddress only when the
        // adapter actually has one, which is exactly the signal we want.
        let has_default_route = Some(!entry.FirstGatewayAddress.is_null());

        adapters.push(NetworkAdapter {
            id,
            name,
            description,
            kind,
            mac_address,
            is_up: entry.OperStatus == IfOperStatusUp,
            speed_mbps,
            addresses: unsafe { collect_addresses(&entry) },
            has_default_route,
        });

        current = entry.Next;
    }

    // Default-route-bearing physical adapters last, so the Network page offers
    // the isolated ones first.
    adapters.sort_by(|a, b| {
        b.kind
            .is_physical()
            .cmp(&a.kind.is_physical())
            .then_with(|| b.is_up.cmp(&a.is_up))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    adapters
}

/// Walks one adapter's unicast address list.
///
/// # Safety
/// `entry` must be a valid adapter record whose backing buffer is alive.
unsafe fn collect_addresses(entry: &IP_ADAPTER_ADDRESSES_LH) -> Vec<IpAddressInfo> {
    let mut addresses = Vec::new();
    let mut current = entry.FirstUnicastAddress;

    while !current.is_null() {
        let unicast = unsafe { core::ptr::read_unaligned(current) };
        let socket_address = unicast.Address;

        if !socket_address.lpSockaddr.is_null() {
            // SAFETY: lpSockaddr points at a sockaddr whose family tells us
            // which concrete type to read.
            let family = unsafe { core::ptr::read_unaligned(socket_address.lpSockaddr) }.sa_family;
            let prefix_length = unicast.OnLinkPrefixLength;

            if family == AF_INET {
                let sockaddr =
                    unsafe { core::ptr::read_unaligned(socket_address.lpSockaddr.cast::<SOCKADDR_IN>()) };
                let octets = unsafe { sockaddr.sin_addr.S_un.S_addr }.to_ne_bytes();
                let address = std::net::Ipv4Addr::from(octets);
                addresses.push(IpAddressInfo {
                    family: IpFamily::Ipv4,
                    address: address.to_string(),
                    netmask: Some(ipv4_netmask(prefix_length).to_string()),
                    prefix_length,
                    broadcast: Some(ipv4_broadcast(address, prefix_length).to_string()),
                });
            } else if family == AF_INET6 {
                let sockaddr = unsafe {
                    core::ptr::read_unaligned(socket_address.lpSockaddr.cast::<SOCKADDR_IN6>())
                };
                let octets = unsafe { sockaddr.sin6_addr.u.Byte };
                addresses.push(IpAddressInfo {
                    family: IpFamily::Ipv6,
                    address: std::net::Ipv6Addr::from(octets).to_string(),
                    netmask: None,
                    prefix_length,
                    broadcast: None,
                });
            }
        }

        current = unicast.Next;
    }

    // IPv4 first: that is what the audio socket binds to.
    addresses.sort_by_key(|info| u8::from(info.family != IpFamily::Ipv4));
    addresses
}

/// Netmask for an IPv4 prefix length.
fn ipv4_netmask(prefix_length: u8) -> std::net::Ipv4Addr {
    let bits = if prefix_length >= 32 {
        u32::MAX
    } else {
        u32::MAX.checked_shl(32 - u32::from(prefix_length)).unwrap_or(0)
    };
    std::net::Ipv4Addr::from(bits)
}

/// Broadcast address for an IPv4 address and prefix length.
fn ipv4_broadcast(address: std::net::Ipv4Addr, prefix_length: u8) -> std::net::Ipv4Addr {
    let host_bits = if prefix_length >= 32 {
        0
    } else {
        u32::MAX.checked_shr(u32::from(prefix_length)).unwrap_or(0)
    };
    std::net::Ipv4Addr::from(u32::from(address) | host_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netmasks_match_their_prefix_lengths() {
        assert_eq!(ipv4_netmask(24).to_string(), "255.255.255.0");
        assert_eq!(ipv4_netmask(16).to_string(), "255.255.0.0");
        assert_eq!(ipv4_netmask(8).to_string(), "255.0.0.0");
        assert_eq!(ipv4_netmask(32).to_string(), "255.255.255.255");
        assert_eq!(ipv4_netmask(0).to_string(), "0.0.0.0");
        // An out-of-range value must not panic.
        assert_eq!(ipv4_netmask(99).to_string(), "255.255.255.255");
    }

    #[test]
    fn broadcast_addresses_are_computed_from_the_prefix() {
        let address = std::net::Ipv4Addr::new(192, 168, 1, 10);
        assert_eq!(ipv4_broadcast(address, 24).to_string(), "192.168.1.255");
        assert_eq!(ipv4_broadcast(address, 16).to_string(), "192.168.255.255");
        assert_eq!(ipv4_broadcast(address, 32).to_string(), "192.168.1.10");
    }
}
