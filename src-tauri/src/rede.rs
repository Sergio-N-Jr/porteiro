//! Endereço da máquina na rede local (IP da LAN e nome `.local`).

use serde::Serialize;
use std::net::IpAddr;

/// Prefixos de interfaces virtuais que não representam a LAN de verdade.
const IFACES_VIRTUAIS: &[&str] = &[
    "lo", "docker", "virbr", "waydroid", "veth", "br-", "vmnet", "vboxnet", "tun", "tap", "wg",
    "tailscale", "zt", "podman", "cni", "flannel", "lxc", "lxd",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InfoRede {
    /// IPv4 da LAN (primeiro de uma interface física), se houver.
    pub ip_lan: Option<String>,
    pub interface: Option<String>,
    pub hostname: String,
}

fn virtual_(nome: &str) -> bool {
    IFACES_VIRTUAIS.iter().any(|p| nome.starts_with(p))
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_else(|_| "localhost".into())
}

pub fn info() -> InfoRede {
    let mut candidatos: Vec<(String, IpAddr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && !virtual_(&i.name))
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_link_local() => Some((i.name, IpAddr::V4(v4))),
            _ => None,
        })
        .collect();
    // Prefere endereços privados (192.168/10/172.16) e cabo antes de wi-fi.
    candidatos.sort_by_key(|(nome, ip)| {
        let privado = matches!(ip, IpAddr::V4(v4) if v4.is_private());
        (!privado, !nome.starts_with('e'), nome.clone())
    });
    let primeiro = candidatos.into_iter().next();
    InfoRede {
        ip_lan: primeiro.as_ref().map(|(_, ip)| ip.to_string()),
        interface: primeiro.map(|(n, _)| n),
        hostname: hostname(),
    }
}
