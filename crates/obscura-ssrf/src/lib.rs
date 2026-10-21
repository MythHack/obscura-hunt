//! The SSRF policy every Obscura transport shares: which IP addresses an
//! outbound request may never reach, and the process-wide env opt-in that
//! lifts the restriction for local testing.
//!
//! This crate has no dependencies on purpose. `obscura-net` (reqwest and wreq
//! transports) and `obscura-render` (the synchronous compatibility loader for
//! images and fonts) both enforce the same deny-set from here, so the checks
//! can never disagree.

use std::net::{IpAddr, Ipv4Addr};

/// Process-wide opt-in via env var. Older flow that issue #4 introduced. The
/// new `--allow-private-network` CLI flag (issue #33) sets a per-client field
/// that is OR'd with this so existing scripts and Docker setups that pin the
/// env var keep working unchanged.
pub fn env_allows_private_network() -> bool {
    matches!(
        std::env::var("OBSCURA_ALLOW_PRIVATE_NETWORK")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// A finite address exemption from the shared deny-set, not an egress firewall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NetworkRange {
    network: IpAddr,
    prefix: u8,
}

impl std::str::FromStr for NetworkRange {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = value.trim().split_once('/')
            .ok_or_else(|| format!("Invalid network CIDR {value:?}: expected address/prefix"))?;
        let network: IpAddr = address.parse().map_err(|_| format!("Invalid network CIDR {value:?}"))?;
        let prefix: u8 = prefix.parse().map_err(|_| format!("Invalid network CIDR {value:?}"))?;
        if prefix > if network.is_ipv4() { 32 } else { 128 } {
            return Err(format!("Invalid network CIDR {value:?}: prefix out of range"));
        }
        Ok(Self { network, prefix })
    }
}

impl NetworkRange {
    pub fn is_unrestricted(&self) -> bool { self.prefix == 0 }

    pub fn contains(&self, address: IpAddr) -> bool {
        let address = match (self.network, address) {
            (IpAddr::V4(_), IpAddr::V6(address)) => address.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(address)),
            (_, address) => address,
        };
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = u32::MAX.checked_shl(u32::from(32 - self.prefix)).unwrap_or(0);
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = u128::MAX.checked_shl(u32::from(128 - self.prefix)).unwrap_or(0);
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}

/// Parse the same comma-separated values accepted by the CLI. Invalid values
/// remain errors so callers cannot silently start with a different policy.
pub fn parse_allowed_networks(value: &str) -> Result<Vec<NetworkRange>, String> {
    value.split(',').filter(|value| !value.trim().is_empty()).map(str::parse).collect()
}

pub fn env_allowed_networks() -> Result<Vec<NetworkRange>, String> {
    std::env::var("OBSCURA_ALLOW_NETWORK").map_or_else(|_| Ok(Vec::new()), |value| parse_allowed_networks(&value))
}

pub fn address_allowed(address: IpAddr, allow_private: bool, ranges: &[NetworkRange]) -> bool {
    allow_private || !is_forbidden_ip(address) || ranges.iter().any(|range| range.contains(address))
}

/// True when `ip` must never be the target of an outbound request from the
/// engine: loopback, RFC1918 private, link-local (incl. the 169.254.169.254
/// cloud-metadata endpoint), broadcast, documentation, the unspecified address
/// (0.0.0.0 / ::, which the OS routes to localhost), IPv6 unique-local
/// (fc00::/7), and any IPv4-mapped/compatible IPv6 form of the above.
/// Centralizes the SSRF deny-set so the literal-host check and the
/// DNS-resolution check (`SsrfGuardResolver`) can never disagree.
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                // std's is_private() covers only RFC1918, so add the IANA
                // special-purpose ranges that also host internal services and
                // are common SSRF targets:
                //   100.64.0.0/10  CGNAT / RFC6598 — cloud metadata (e.g.
                //                  Alibaba 100.100.100.200) lives here.
                //   198.18.0.0/15  benchmarking / RFC2544.
                //   192.88.99.0/24 6to4 relay anycast / RFC7526.
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                // Most of 192.0.0.0/24 is special-purpose and not globally
                // reachable. Keep the two globally reachable PCP anycast
                // assignments usable rather than blocking the entire /24.
                || (o[0] == 192
                    && o[1] == 0
                    && o[2] == 0
                    && o[3] != 9
                    && o[3] != 10)
                // 240.0.0.0/4 is reserved (255.255.255.255 was already
                // covered by is_broadcast()).
                || o[0] >= 240
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
            {
                return true;
            }
            // Unwrap IPv4-mapped (::ffff:a.b.c.d) and IPv4-compatible (::a.b.c.d)
            // forms and re-check the embedded v4 so e.g. [::ffff:127.0.0.1] or
            // [::ffff:169.254.169.254] cannot slip past the v6 arm.
            if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
                return is_forbidden_ip(IpAddr::V4(v4));
            }

            let s = v6.segments();
            // IPv4/IPv6 translation prefix (RFC 6052). Only /96 has a fixed
            // embedded-address position; the local-use /48 is therefore
            // blocked outright below.
            if s[0] == 0x64
                && s[1] == 0xff9b
                && s[2] == 0
                && s[3] == 0
                && s[4] == 0
                && s[5] == 0
            {
                return is_forbidden_ip(IpAddr::V4(Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                )));
            }
            // 6to4 carries its IPv4 endpoint in bits 16..48.
            if s[0] == 0x2002 {
                return is_forbidden_ip(IpAddr::V4(Ipv4Addr::new(
                    (s[1] >> 8) as u8,
                    s[1] as u8,
                    (s[2] >> 8) as u8,
                    s[2] as u8,
                )));
            }
            // Teredo (RFC 4380, 2001:0::/32) carries the server's IPv4 in
            // bits 32..64 and the client's public IPv4, bit-inverted, in the
            // last 32 bits. Either one embedding a forbidden address is
            // refused, like the other IPv4-in-IPv6 forms above.
            if s[0] == 0x2001 && s[1] == 0 {
                let server = Ipv4Addr::new(
                    (s[2] >> 8) as u8,
                    s[2] as u8,
                    (s[3] >> 8) as u8,
                    s[3] as u8,
                );
                let client = Ipv4Addr::new(
                    !(s[6] >> 8) as u8,
                    !s[6] as u8,
                    !(s[7] >> 8) as u8,
                    !s[7] as u8,
                );
                return is_forbidden_ip(IpAddr::V4(server)) || is_forbidden_ip(IpAddr::V4(client));
            }

            // Discard-only, local-use NAT64, and documentation prefixes.
            (s[0] == 0x100 && s[1] == 0 && s[2] == 0 && s[3] == 0)
                || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1)
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || (s[0] == 0x3fff && s[1] & 0xf000 == 0)
        }
    }
}
