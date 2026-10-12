//! mDNS/DNS-SD service advertisement.
//!
//! ## Phase status
//!
//! The service type and TXT-record contract below are **frozen** — StageLink
//! Mobile browses for exactly this. The prose version lives in the "Service
//! discovery" section of `docs/protocol/pairing.md`; there is no separate
//! discovery document. Actually publishing the record is Phase 5 work and will
//! be behind an explicit `publish()` call so a development build never
//! announces itself on a venue network by accident.
//!
//! Security note that belongs with the contract, not just the implementation:
//! discovery is advisory. A receiver that finds this record still has to
//! complete pairing and authenticate. Nothing in a TXT record is trusted, and
//! nothing in it is secret — no tokens, no keys, no pairing codes.

use serde::{Deserialize, Serialize};

/// DNS-SD service type. Frozen: StageLink Mobile browses for this exact string.
pub const SERVICE_TYPE: &str = "_stagelink._udp.local.";

/// TXT record keys. Frozen alongside the service type.
pub const TXT_KEY_PROTOCOL_VERSION: &str = "cpv";
pub const TXT_KEY_CONTROL_PORT: &str = "cport";
pub const TXT_KEY_STREAM_PORT: &str = "sport";
pub const TXT_KEY_AUDIO_PROTOCOL: &str = "apv";
pub const TXT_KEY_SERVER_NAME: &str = "name";
pub const TXT_KEY_KEY_FINGERPRINT: &str = "fp";
pub const TXT_KEY_REQUIRES_PAIRING: &str = "pair";

/// What the desktop advertises about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAdvertisement {
    /// Instance name shown to the operator, e.g. "StageLink Core".
    pub instance_name: String,
    /// Hostname used in the SRV record.
    pub hostname: String,
    /// IPv4 address receivers should connect to.
    pub address: String,
    pub control_port: u16,
    pub stream_port: u16,
    pub control_protocol_version: String,
    pub audio_protocol_version: u8,
    /// Fingerprint of the desktop's long-term public key, so a receiver can
    /// detect that it is talking to a *different* desktop than last time.
    /// A fingerprint is a hash, not a secret.
    pub key_fingerprint: String,
    /// Always true: a join code is the only way in. Advertised so a receiver
    /// that finds a desktop it has never joined can say "scan the code on the
    /// operator's screen" rather than failing with a bare connection error.
    pub requires_pairing: bool,
}

impl ServiceAdvertisement {
    /// The port that goes in the SRV record.
    ///
    /// **This is the audio UDP port, not the control port.** The service type
    /// is `_stagelink._udp`, and an SRV record carrying a TCP port under a
    /// `_udp` type would be wrong in a way that only breaks for whichever
    /// third-party browser takes the record at face value.
    ///
    /// A receiver must therefore read [`TXT_KEY_CONTROL_PORT`] for the control
    /// channel and must not connect it to the SRV port. The redundancy between
    /// the SRV port and `sport` is deliberate: the TXT value is what a
    /// receiver is told to use, and the SRV record is what the protocol
    /// requires to be populated.
    #[must_use]
    pub const fn srv_port(&self) -> u16 {
        self.stream_port
    }

    /// Builds the TXT record properties.
    ///
    /// Deliberately excludes anything secret. If a future field would need to
    /// be secret, it does not belong in discovery at all.
    #[must_use]
    pub fn txt_properties(&self) -> Vec<(&'static str, String)> {
        vec![
            (TXT_KEY_PROTOCOL_VERSION, self.control_protocol_version.clone()),
            (TXT_KEY_AUDIO_PROTOCOL, self.audio_protocol_version.to_string()),
            (TXT_KEY_CONTROL_PORT, self.control_port.to_string()),
            (TXT_KEY_STREAM_PORT, self.stream_port.to_string()),
            (TXT_KEY_SERVER_NAME, self.instance_name.clone()),
            (TXT_KEY_KEY_FINGERPRINT, self.key_fingerprint.clone()),
            (
                TXT_KEY_REQUIRES_PAIRING,
                if self.requires_pairing { "1" } else { "0" }.to_owned(),
            ),
        ]
    }

    /// Validates the advertisement before it is published.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.instance_name.trim().is_empty() {
            return Err("the instance name must not be empty");
        }
        // DNS-SD instance names are limited to 63 bytes.
        if self.instance_name.len() > 63 {
            return Err("the instance name must be 63 bytes or fewer");
        }
        if self.address.parse::<std::net::Ipv4Addr>().is_err() {
            return Err("the advertised address must be a valid IPv4 address");
        }
        if self.control_port < 1_024 || self.stream_port < 1_024 {
            return Err("advertised ports must be above 1023");
        }
        if self.control_port == self.stream_port {
            return Err("the control and stream ports must differ");
        }
        if self.key_fingerprint.trim().is_empty() {
            return Err("a key fingerprint is required so receivers can pin this desktop");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertisement() -> ServiceAdvertisement {
        ServiceAdvertisement {
            instance_name: "StageLink Core".to_owned(),
            hostname: "stage-laptop.local.".to_owned(),
            address: "10.0.0.1".to_owned(),
            control_port: 45_871,
            stream_port: 45_872,
            control_protocol_version: "1.0.0".to_owned(),
            audio_protocol_version: 1,
            key_fingerprint: "AB:CD:EF:01".to_owned(),
            requires_pairing: true,
        }
    }

    #[test]
    fn the_service_type_is_frozen() {
        // StageLink Mobile browses for this exact string; changing it is a
        // breaking protocol change, not a refactor.
        assert_eq!(SERVICE_TYPE, "_stagelink._udp.local.");
    }

    /// Pinned because a publisher reaching for "the port" would reasonably
    /// pick the control port, and the mistake is invisible to our own
    /// receiver — which reads `cport` — while being wrong for anything else
    /// that browses the record.
    #[test]
    fn the_srv_port_is_the_audio_port_to_match_the_udp_service_type() {
        let advertisement = advertisement();
        assert_eq!(advertisement.srv_port(), advertisement.stream_port);
        assert_ne!(advertisement.srv_port(), advertisement.control_port);
        assert!(SERVICE_TYPE.contains("._udp."));
    }

    #[test]
    fn txt_properties_carry_everything_a_receiver_needs_to_connect() {
        let properties = advertisement().txt_properties();
        let lookup = |key: &str| {
            properties
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(lookup(TXT_KEY_CONTROL_PORT), Some("45871".to_owned()));
        assert_eq!(lookup(TXT_KEY_STREAM_PORT), Some("45872".to_owned()));
        assert_eq!(lookup(TXT_KEY_PROTOCOL_VERSION), Some("1.0.0".to_owned()));
        assert_eq!(lookup(TXT_KEY_AUDIO_PROTOCOL), Some("1".to_owned()));
        assert_eq!(lookup(TXT_KEY_REQUIRES_PAIRING), Some("1".to_owned()));
    }

    #[test]
    fn no_secret_ever_appears_in_a_txt_record() {
        let properties = advertisement().txt_properties();
        for (key, value) in &properties {
            let lowered = format!("{key} {value}").to_lowercase();
            for forbidden in ["token", "secret", "password", "privatekey", "pairingcode"] {
                assert!(
                    !lowered.contains(forbidden),
                    "TXT record must not contain {forbidden}: {key}={value}"
                );
            }
        }
    }

    #[test]
    fn a_well_formed_advertisement_validates() {
        advertisement().validate().unwrap();
    }

    #[test]
    fn an_empty_or_overlong_instance_name_is_refused() {
        let empty = ServiceAdvertisement { instance_name: "  ".to_owned(), ..advertisement() };
        assert!(empty.validate().is_err());

        let long = ServiceAdvertisement { instance_name: "x".repeat(64), ..advertisement() };
        assert!(long.validate().is_err());
    }

    #[test]
    fn a_non_address_is_refused() {
        let bad = ServiceAdvertisement { address: "stage-laptop".to_owned(), ..advertisement() };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn duplicate_or_privileged_ports_are_refused() {
        let duplicate = ServiceAdvertisement { stream_port: 45_871, ..advertisement() };
        assert!(duplicate.validate().is_err());

        let privileged = ServiceAdvertisement { control_port: 80, ..advertisement() };
        assert!(privileged.validate().is_err());
    }

    #[test]
    fn a_missing_fingerprint_is_refused_so_receivers_can_always_pin() {
        let bare = ServiceAdvertisement { key_fingerprint: String::new(), ..advertisement() };
        assert!(bare.validate().is_err());
    }
}
