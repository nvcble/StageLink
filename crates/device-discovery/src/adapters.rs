//! Network adapter enumeration and operator-facing warnings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterKind {
    Ethernet,
    Wifi,
    Loopback,
    Virtual,
    Other,
}

impl AdapterKind {
    /// Whether this is real hardware a stage network could run over.
    #[must_use]
    pub const fn is_physical(self) -> bool {
        matches!(self, Self::Ethernet | Self::Wifi)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IpFamily {
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpAddressInfo {
    pub family: IpFamily,
    pub address: String,
    pub netmask: Option<String>,
    pub prefix_length: u8,
    pub broadcast: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAdapter {
    /// OS adapter name (a GUID on Windows). Stable across reboots.
    pub id: String,
    /// Friendly name, e.g. "Ethernet 2".
    pub name: String,
    pub description: String,
    pub kind: AdapterKind,
    pub mac_address: Option<String>,
    pub is_up: bool,
    pub speed_mbps: Option<u32>,
    pub addresses: Vec<IpAddressInfo>,
    /// Whether the adapter has a gateway. `None` when not determined.
    ///
    /// StageLink never *requires* this to be true — the recommended setup is an
    /// isolated access point with no uplink. It is surfaced so the operator can
    /// confirm they picked the right adapter.
    pub has_default_route: Option<bool>,
}

impl NetworkAdapter {
    /// First IPv4 address, which is what audio is bound to.
    #[must_use]
    pub fn primary_ipv4(&self) -> Option<&IpAddressInfo> {
        self.addresses.iter().find(|info| info.family == IpFamily::Ipv4)
    }

    /// Whether this adapter could carry a stream right now.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.is_up && self.primary_ipv4().is_some() && self.kind != AdapterKind::Loopback
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NetworkWarningCode {
    NoAdapterSelected,
    SelectedAdapterMissing,
    StreamingOverWifi,
    NonPhysicalAdapter,
    SharedWithInternet,
    SubnetConflict,
    AdapterDown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkWarning {
    pub code: NetworkWarningCode,
    pub severity: Severity,
    pub message: String,
    /// Concrete action for the operator, not a restatement of the problem.
    pub remediation: Option<String>,
}

/// Produces the warnings shown on the Network page.
///
/// Note what is *not* here: nothing about internet access being absent. An
/// isolated access point with no uplink is the recommended configuration, so
/// treating it as a problem would be wrong.
#[must_use]
pub fn evaluate_warnings(
    adapters: &[NetworkAdapter],
    selected_adapter_id: Option<&str>,
) -> Vec<NetworkWarning> {
    let mut warnings = Vec::new();

    let Some(selected_id) = selected_adapter_id else {
        warnings.push(NetworkWarning {
            code: NetworkWarningCode::NoAdapterSelected,
            severity: Severity::Warning,
            message: "No network adapter has been chosen for streaming.".to_owned(),
            remediation: Some(
                "Pick the adapter cabled to your Wi-Fi access point, usually an Ethernet one."
                    .to_owned(),
            ),
        });
        return warnings;
    };

    let Some(selected) = adapters.iter().find(|adapter| adapter.id == selected_id) else {
        warnings.push(NetworkWarning {
            code: NetworkWarningCode::SelectedAdapterMissing,
            severity: Severity::Error,
            message: "The saved streaming adapter is no longer present on this machine."
                .to_owned(),
            remediation: Some("Choose an adapter that is currently connected.".to_owned()),
        });
        return warnings;
    };

    if !selected.is_up {
        warnings.push(NetworkWarning {
            code: NetworkWarningCode::AdapterDown,
            severity: Severity::Error,
            message: format!("\"{}\" is not connected.", selected.name),
            remediation: Some("Check the cable between this machine and the access point.".to_owned()),
        });
    }

    if !selected.kind.is_physical() {
        warnings.push(NetworkWarning {
            code: NetworkWarningCode::NonPhysicalAdapter,
            severity: Severity::Error,
            message: format!(
                "\"{}\" is a loopback or virtual adapter and cannot reach receivers.",
                selected.name
            ),
            remediation: Some("Choose a physical Ethernet or Wi-Fi adapter.".to_owned()),
        });
    }

    if selected.kind == AdapterKind::Wifi {
        warnings.push(NetworkWarning {
            code: NetworkWarningCode::StreamingOverWifi,
            severity: Severity::Warning,
            message: "Streaming from this machine over Wi-Fi puts the desktop on the same \
                      contended radio as the receivers."
                .to_owned(),
            remediation: Some(
                "Cable this machine to the access point over Ethernet and select that adapter."
                    .to_owned(),
            ),
        });
    }

    if selected.has_default_route == Some(true) {
        warnings.push(NetworkWarning {
            code: NetworkWarningCode::SharedWithInternet,
            severity: Severity::Info,
            message: format!(
                "\"{}\" has a route to the internet, so the access point is probably not isolated.",
                selected.name
            ),
            remediation: Some(
                "A dedicated access point with no uplink gives the most predictable audio."
                    .to_owned(),
            ),
        });
    }

    // Two adapters on the same /24 make the OS routing choice ambiguous, which
    // shows up as receivers that pair but never get audio.
    if let Some(selected_ipv4) = selected.primary_ipv4() {
        let clashing: Vec<&str> = adapters
            .iter()
            .filter(|adapter| adapter.id != selected.id && adapter.is_up)
            .filter(|adapter| {
                adapter
                    .primary_ipv4()
                    .is_some_and(|other| same_subnet(&selected_ipv4.address, &other.address))
            })
            .map(|adapter| adapter.name.as_str())
            .collect();

        if !clashing.is_empty() {
            warnings.push(NetworkWarning {
                code: NetworkWarningCode::SubnetConflict,
                severity: Severity::Warning,
                message: format!(
                    "\"{}\" shares a subnet with {}. Windows may route audio out of the wrong adapter.",
                    selected.name,
                    clashing.join(", ")
                ),
                remediation: Some(
                    "Put the stage network on its own subnet, or disable the other adapter during \
                     the service."
                        .to_owned(),
                ),
            });
        }
    }

    warnings
}

/// Whether two IPv4 addresses share the first three octets.
///
/// A deliberate approximation: a true comparison needs both netmasks, and the
/// point of this check is to flag an ambiguous-looking setup for a human, not
/// to compute routing.
fn same_subnet(left: &str, right: &str) -> bool {
    fn prefix(address: &str) -> Option<String> {
        let octets: Vec<&str> = address.split('.').collect();
        if octets.len() != 4 {
            return None;
        }
        Some(octets[..3].join("."))
    }
    match (prefix(left), prefix(right)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(windows)]
mod windows_impl;

/// Enumerates the machine's network adapters.
#[cfg(windows)]
pub fn enumerate_adapters() -> Result<Vec<NetworkAdapter>, crate::adapters::AdapterError> {
    windows_impl::enumerate()
}

#[cfg(not(windows))]
pub fn enumerate_adapters() -> Result<Vec<NetworkAdapter>, AdapterError> {
    Err(AdapterError::UnsupportedPlatform)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    #[error("adapter enumeration is not implemented on this platform")]
    UnsupportedPlatform,

    #[error("GetAdaptersAddresses failed with code {0}")]
    SystemCall(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4(address: &str) -> IpAddressInfo {
        IpAddressInfo {
            family: IpFamily::Ipv4,
            address: address.to_owned(),
            netmask: Some("255.255.255.0".to_owned()),
            prefix_length: 24,
            broadcast: None,
        }
    }

    fn adapter(id: &str, name: &str, kind: AdapterKind, address: Option<&str>) -> NetworkAdapter {
        NetworkAdapter {
            id: id.to_owned(),
            name: name.to_owned(),
            description: name.to_owned(),
            kind,
            mac_address: Some("00:11:22:33:44:55".to_owned()),
            is_up: true,
            speed_mbps: Some(1_000),
            addresses: address.map(|a| vec![ipv4(a)]).unwrap_or_default(),
            has_default_route: Some(false),
        }
    }

    #[test]
    fn no_selection_prompts_the_operator_to_choose() {
        let warnings = evaluate_warnings(&[], None);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, NetworkWarningCode::NoAdapterSelected);
        assert!(warnings[0].remediation.is_some());
    }

    #[test]
    fn a_vanished_adapter_is_an_error() {
        let adapters = vec![adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("10.0.0.1"))];
        let warnings = evaluate_warnings(&adapters, Some("usb-dongle-that-was-unplugged"));
        assert_eq!(warnings[0].code, NetworkWarningCode::SelectedAdapterMissing);
        assert_eq!(warnings[0].severity, Severity::Error);
    }

    #[test]
    fn a_clean_isolated_ethernet_setup_produces_no_warnings() {
        let adapters = vec![adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("10.0.0.1"))];
        let warnings = evaluate_warnings(&adapters, Some("eth0"));
        assert!(
            warnings.is_empty(),
            "the recommended setup must not be flagged: {warnings:?}"
        );
    }

    #[test]
    fn a_missing_internet_uplink_is_never_treated_as_a_problem() {
        let mut ethernet = adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("10.0.0.1"));
        ethernet.has_default_route = Some(false);
        let warnings = evaluate_warnings(&[ethernet], Some("eth0"));
        assert!(
            warnings.is_empty(),
            "StageLink does not need the internet for live audio"
        );
    }

    #[test]
    fn streaming_over_wifi_is_a_warning_not_a_refusal() {
        let adapters = vec![adapter("wlan0", "Wi-Fi", AdapterKind::Wifi, Some("192.168.1.5"))];
        let warnings = evaluate_warnings(&adapters, Some("wlan0"));
        let warning = warnings
            .iter()
            .find(|w| w.code == NetworkWarningCode::StreamingOverWifi)
            .expect("Wi-Fi should be flagged");
        assert_eq!(warning.severity, Severity::Warning);
        assert!(warning.remediation.as_ref().unwrap().contains("Ethernet"));
    }

    #[test]
    fn a_loopback_selection_is_an_error() {
        let adapters = vec![adapter("lo", "Loopback", AdapterKind::Loopback, Some("127.0.0.1"))];
        let warnings = evaluate_warnings(&adapters, Some("lo"));
        assert!(
            warnings
                .iter()
                .any(|w| w.code == NetworkWarningCode::NonPhysicalAdapter
                    && w.severity == Severity::Error)
        );
    }

    #[test]
    fn a_down_adapter_is_reported_with_a_cable_hint() {
        let mut ethernet = adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("10.0.0.1"));
        ethernet.is_up = false;
        let warnings = evaluate_warnings(&[ethernet], Some("eth0"));
        let warning = warnings
            .iter()
            .find(|w| w.code == NetworkWarningCode::AdapterDown)
            .unwrap();
        assert!(warning.remediation.as_ref().unwrap().contains("cable"));
    }

    #[test]
    fn an_internet_route_is_informational_only() {
        let mut ethernet = adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("10.0.0.1"));
        ethernet.has_default_route = Some(true);
        let warnings = evaluate_warnings(&[ethernet], Some("eth0"));
        let warning = warnings
            .iter()
            .find(|w| w.code == NetworkWarningCode::SharedWithInternet)
            .unwrap();
        assert_eq!(
            warning.severity,
            Severity::Info,
            "an uplink is a preference, not a fault"
        );
    }

    #[test]
    fn overlapping_subnets_are_flagged() {
        let adapters = vec![
            adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("192.168.1.10")),
            adapter("wlan0", "Wi-Fi", AdapterKind::Wifi, Some("192.168.1.20")),
        ];
        let warnings = evaluate_warnings(&adapters, Some("eth0"));
        let warning = warnings
            .iter()
            .find(|w| w.code == NetworkWarningCode::SubnetConflict)
            .expect("a shared subnet should be flagged");
        assert!(warning.message.contains("Wi-Fi"));
    }

    #[test]
    fn distinct_subnets_are_not_flagged() {
        let adapters = vec![
            adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("10.0.0.1")),
            adapter("wlan0", "Wi-Fi", AdapterKind::Wifi, Some("192.168.1.20")),
        ];
        let warnings = evaluate_warnings(&adapters, Some("eth0"));
        assert!(!warnings.iter().any(|w| w.code == NetworkWarningCode::SubnetConflict));
    }

    #[test]
    fn usability_requires_an_address_and_a_link() {
        let mut ethernet = adapter("eth0", "Ethernet", AdapterKind::Ethernet, Some("10.0.0.1"));
        assert!(ethernet.is_usable());

        ethernet.addresses.clear();
        assert!(!ethernet.is_usable(), "no address means no streaming");

        let down = NetworkAdapter { is_up: false, ..adapter("e", "E", AdapterKind::Ethernet, Some("10.0.0.2")) };
        assert!(!down.is_usable());
    }

    /// Hardware-dependent: asserts the shape of what the OS returns rather than
    /// the presence of a particular adapter.
    #[cfg(windows)]
    #[test]
    fn enumeration_returns_consistent_adapter_records() {
        let adapters = enumerate_adapters().expect("adapter enumeration must not fail");
        assert!(
            !adapters.is_empty(),
            "every Windows machine has at least a loopback adapter"
        );
        for adapter in &adapters {
            assert!(!adapter.id.is_empty());
            assert!(!adapter.name.is_empty());
            for address in &adapter.addresses {
                assert!(matches!(address.family, IpFamily::Ipv4 | IpFamily::Ipv6));
                assert!(!address.address.is_empty());
                assert!(address.prefix_length <= 128);
            }
        }
        // There should be at least one loopback.
        assert!(
            adapters.iter().any(|a| a.kind == AdapterKind::Loopback),
            "loopback should be reported"
        );
    }
}
