//! Local address discovery for SIP signalling.
//!
//! The engine must bind to the interface that actually reaches the registrar.
//! Binding to loopback (the SIP stack's default) makes every send to a LAN
//! address fail with `EINVAL`, and binding to `0.0.0.0` leaves the stack
//! without a concrete address to advertise in `Via`/`Contact`.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};

use crate::error::{Error, Result};

/// Resolves `host:port` to a single socket address, preferring IPv4.
///
/// IPv4 is preferred because home PBXs (the Fritz!Box included) commonly
/// publish an IPv6 address that does not accept SIP, while their IPv4 LAN
/// address always does.
pub fn resolve(host: &str, port: u16) -> Result<SocketAddr> {
    let candidates: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::Resolve { host: format!("{host}: {e}") })?
        .collect();

    candidates
        .iter()
        .find(|addr| addr.is_ipv4())
        .or_else(|| candidates.first())
        .copied()
        .ok_or_else(|| Error::Resolve { host: host.to_string() })
}

/// Returns the local IP the kernel would use to reach `peer`.
///
/// Connecting a UDP socket only sets the default destination; it sends no
/// packets, so this is a cheap, side-effect-free routing lookup.
pub fn local_ip_towards(peer: SocketAddr) -> Result<IpAddr> {
    let bind: SocketAddr = if peer.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    };

    let socket = UdpSocket::bind(bind)?;
    socket.connect(peer)?;
    Ok(socket.local_addr()?.ip())
}

/// Picks the local address to bind SIP to.
///
/// Tries `preferred_port` (5060 by convention, which some PBXs expect for
/// inbound requests) and falls back to an ephemeral port when it is taken —
/// a second softphone on the machine should not prevent registration.
pub fn bind_address(peer: SocketAddr, preferred_port: u16) -> Result<SocketAddr> {
    let ip = local_ip_towards(peer)?;
    let preferred = SocketAddr::new(ip, preferred_port);

    // Probe only: the socket is dropped either way, freeing the port for the
    // SIP stack to bind for real.
    if let Ok(socket) = UdpSocket::bind(preferred) {
        drop(socket);
        return Ok(preferred);
    }

    let socket = UdpSocket::bind(SocketAddr::new(ip, 0))?;
    let addr = socket.local_addr()?;
    drop(socket);
    Ok(addr)
}
