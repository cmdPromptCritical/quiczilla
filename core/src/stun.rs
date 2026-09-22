//! Minimal STUN binding discovery used to obtain a NAT's public UDP candidate.
//!
//! This implements only RFC 8489 Binding requests.  Relaying is deliberately
//! outside this module; TURN is a separate service and transport choice.

use anyhow::{Context, Result};
use ring::rand::{SecureRandom, SystemRandom};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

const MAGIC_COOKIE: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StunCandidate {
    pub local_port: u16,
    pub public_addr: SocketAddr,
    pub local_addr: Option<IpAddr>,
}

/// Discover the server-reflexive UDP address while reserving `local_port`.
/// The caller must bind its subsequent QUIC connection to the returned local
/// port promptly, since NAT mappings are inherently short lived.
pub fn discover(server: SocketAddr, local_port: u16) -> Result<StunCandidate> {
    let bind_addr = match server {
        SocketAddr::V4(_) => format!("0.0.0.0:{local_port}"),
        SocketAddr::V6(_) => format!("[::]:{local_port}"),
    };
    let socket = UdpSocket::bind(&bind_addr)
        .with_context(|| format!("Failed to bind UDP socket for STUN on {bind_addr}"))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;

    let mut transaction_id = [0u8; 12];
    SystemRandom::new()
        .fill(&mut transaction_id)
        .map_err(|_| anyhow::anyhow!("Failed to generate STUN transaction ID"))?;

    let mut request = [0u8; 20];
    request[..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    request[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    request[8..].copy_from_slice(&transaction_id);
    socket.connect(server)?;
    let actual_local_ip = socket.local_addr().map(|a| a.ip()).ok();
    // disconnect the socket so it can send/recv freely if needed, wait, UDP connect is fine,
    // but MsQuic will create its own socket.
    socket
        .send(&request)
        .with_context(|| format!("Failed to send STUN request to {server}"))?;

    let mut response = [0u8; 1024];
    let len = socket
        .recv(&mut response)
        .with_context(|| format!("No STUN response from {server}"))?;
    let public_addr = parse_binding_response(&response[..len], &transaction_id)?;
    Ok(StunCandidate {
        local_port: socket.local_addr()?.port(),
        public_addr,
        local_addr: actual_local_ip,
    })
}

fn parse_binding_response(message: &[u8], transaction_id: &[u8; 12]) -> Result<SocketAddr> {
    if message.len() < 20 || u16::from_be_bytes([message[0], message[1]]) != BINDING_SUCCESS {
        anyhow::bail!("Invalid STUN binding response");
    }
    if message[4..8] != MAGIC_COOKIE.to_be_bytes() || message[8..20] != *transaction_id {
        anyhow::bail!("STUN response transaction did not match request");
    }
    let declared_len = u16::from_be_bytes([message[2], message[3]]) as usize;
    if message.len() < 20 + declared_len {
        anyhow::bail!("Truncated STUN binding response");
    }

    let mut offset = 20;
    while offset + 4 <= 20 + declared_len {
        let kind = u16::from_be_bytes([message[offset], message[offset + 1]]);
        let length = u16::from_be_bytes([message[offset + 2], message[offset + 3]]) as usize;
        let value_start = offset + 4;
        let value_end = value_start + length;
        if value_end > message.len() {
            anyhow::bail!("Truncated STUN attribute");
        }
        if kind == XOR_MAPPED_ADDRESS && length >= 8 {
            let family = message[value_start + 1];
            let port = u16::from_be_bytes([message[value_start + 2], message[value_start + 3]])
                ^ (MAGIC_COOKIE >> 16) as u16;
            let address = match family {
                0x01 if length >= 8 => {
                    let cookie = MAGIC_COOKIE.to_be_bytes();
                    IpAddr::from([
                        message[value_start + 4] ^ cookie[0],
                        message[value_start + 5] ^ cookie[1],
                        message[value_start + 6] ^ cookie[2],
                        message[value_start + 7] ^ cookie[3],
                    ])
                }
                0x02 if length >= 20 => {
                    let mut bytes = [0u8; 16];
                    let cookie = MAGIC_COOKIE.to_be_bytes();
                    for index in 0..16 {
                        let mask = if index < 4 {
                            cookie[index]
                        } else {
                            transaction_id[index - 4]
                        };
                        bytes[index] = message[value_start + 4 + index] ^ mask;
                    }
                    IpAddr::from(bytes)
                }
                _ => {
                    offset = value_end + ((4 - length % 4) % 4);
                    continue;
                }
            };
            return Ok(SocketAddr::new(address, port));
        }
        offset = value_end + ((4 - length % 4) % 4);
    }
    anyhow::bail!("STUN response did not include XOR-MAPPED-ADDRESS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xor_mapped_ipv4_address() {
        let tx = [7u8; 12];
        let mut response = vec![1, 1, 0, 12, 0x21, 0x12, 0xA4, 0x42];
        response.extend_from_slice(&tx);
        response.extend_from_slice(&[0, 0x20, 0, 8, 0, 1, 0x21, 0x1e, 0x21, 0x12, 0xA4, 0x43]);
        assert_eq!(
            parse_binding_response(&response, &tx).unwrap(),
            "0.0.0.1:12".parse().unwrap()
        );
    }
}
