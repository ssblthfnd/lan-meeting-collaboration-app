//! Which address to tell participants to use.
//!
//! The Host binds `0.0.0.0` and so is reachable on every interface, but the join
//! URL can only name one. Picking it is a decision only the Host can make:
//! a laptop may be on Wi-Fi and Ethernet and a VPN at once, and only the person
//! in the room knows which network the participants are on.
//!
//! So this module enumerates and describes; it does not choose. Architecture
//! rules section 4 is explicit that `127.0.0.1` must never be assumed reachable
//! by a participant, and [`Interface::is_loopback`] exists so the Host UI can say
//! so rather than silently offering an address that cannot work.

use std::net::IpAddr;

/// One address the Host could advertise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// The interface name as the operating system reports it.
    pub name: String,
    pub address: IpAddr,
    /// Loopback reaches only this machine. Offered, clearly marked, because it
    /// is genuinely useful for trying the participant UI on the Host itself.
    pub is_loopback: bool,
    /// A private-range address, which is what a LAN normally uses. The Host UI
    /// suggests these first.
    pub is_private: bool,
}

/// Every IPv4 address this machine has, most useful first.
///
/// IPv4 only, deliberately: a join URL is read off a screen or scanned from a
/// QR code, and an IPv6 literal in a URL needs brackets and is far longer. Home
/// and office LANs route IPv4, so nothing is lost by keeping the URL short.
///
/// Ordered so a private LAN address comes before anything else and loopback
/// comes last, with the interface name as a tiebreaker so the list is stable
/// between calls (architecture rules section 26.4 applies to any list a person
/// reads).
#[must_use]
pub fn interfaces() -> Vec<Interface> {
    let mut found: Vec<Interface> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, address)| match address {
            IpAddr::V4(v4) => Some(Interface {
                name,
                address,
                is_loopback: v4.is_loopback(),
                is_private: v4.is_private(),
            }),
            IpAddr::V6(_) => None,
        })
        .collect();

    found.sort_by(|a, b| {
        // Private LAN addresses first, loopback last, then by name and address
        // so repeated calls agree.
        let rank = |i: &Interface| match (i.is_private, i.is_loopback) {
            (true, false) => 0,
            (false, false) => 1,
            _ => 2,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.address.cmp(&b.address))
    });

    found
}

/// The address the Host is most likely to want, if there is an obvious one.
///
/// A *suggestion* for the UI to preselect, never a silent choice: the Host still
/// sees which address the join URL names, and can change it.
#[must_use]
pub fn suggested() -> Option<Interface> {
    let all = interfaces();
    all.iter()
        .find(|i| i.is_private && !i.is_loopback)
        .or_else(|| all.first())
        .cloned()
}

/// Assemble the URL a participant opens.
///
/// Built here so the token appears in exactly one format, and so the Host UI
/// cannot assemble a subtly different one for the QR code than for the text.
#[must_use]
pub fn join_url(address: IpAddr, port: u16, token: &str) -> String {
    format!("http://{address}:{port}/join/{token}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn a_join_url_has_one_shape() {
        assert_eq!(
            join_url(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)), 8765, "abc123"),
            "http://192.168.1.42:8765/join/abc123"
        );
    }

    #[test]
    fn enumeration_is_stable_and_marks_loopback() {
        // Whatever this machine has, the answer must not change between calls -
        // a Host choosing an address from a list that reorders itself would be
        // choosing something different each time.
        let first = interfaces();
        assert_eq!(first, interfaces());

        for interface in &first {
            let IpAddr::V4(v4) = interface.address else {
                panic!("IPv6 must be filtered out");
            };
            assert_eq!(interface.is_loopback, v4.is_loopback());
            assert_eq!(interface.is_private, v4.is_private());
        }

        // Loopback, if present, is last: it reaches only this machine, and
        // section 4 forbids assuming a participant can use it.
        if let Some(position) = first.iter().position(|i| i.is_loopback) {
            assert!(
                first[position..].iter().all(|i| i.is_loopback),
                "loopback addresses must sort last"
            );
        }
    }

    #[test]
    fn the_suggestion_prefers_a_private_lan_address() {
        if let Some(suggested) = suggested() {
            let all = interfaces();
            if all.iter().any(|i| i.is_private && !i.is_loopback) {
                assert!(suggested.is_private && !suggested.is_loopback);
            }
        }
    }
}
