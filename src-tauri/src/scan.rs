//! Descoberta de portas TCP em escuta lendo `/proc/net/tcp{,6}` e `/proc/<pid>/fd`.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Estado TCP_LISTEN no `/proc/net/tcp`.
const TCP_LISTEN: &str = "0A";
const TAM_MAX_CMDLINE: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Escopo {
    /// Só aceita conexões da própria máquina (127.x / ::1).
    Local,
    /// Aceita conexões vindas da rede (0.0.0.0, ::, IP da LAN).
    Rede,
}

/// Um socket em escuta, como aparece no `/proc/net/tcp*`.
#[derive(Debug, Clone)]
struct Socket {
    ip: IpAddr,
    porta: u16,
    uid: u32,
    inode: u64,
}

/// Informações do processo dono de um socket.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Processo {
    pub pid: u32,
    pub programa: String,
    /// Nome do executável (`/proc/<pid>/exe`), útil quando o `comm` é nome de thread.
    pub executavel: Option<String>,
    pub cmdline: String,
    pub pasta: Option<String>,
}

/// Uma porta em escuta (agrupando IPv4/IPv6 e múltiplos binds da mesma porta).
#[derive(Debug, Clone, Serialize)]
pub struct Porta {
    pub porta: u16,
    /// Endereços de bind, ex.: `127.0.0.1`, `0.0.0.0`, `[::]`.
    pub enderecos: Vec<String>,
    pub escopo: Escopo,
    /// `None` quando o processo é de outro usuário (ou root) e não dá para ler.
    pub processo: Option<Processo>,
    /// `true` quando o socket pertence a outro usuário.
    pub outro_usuario: bool,
}

pub fn escopo_do_ip(ip: &IpAddr) -> Escopo {
    let loopback = match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    };
    if loopback {
        Escopo::Local
    } else {
        Escopo::Rede
    }
}

fn formatar_ip(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => format!("[{v6}]"),
        },
    }
}

/// Converte o endereço hexadecimal do kernel (little-endian por palavra de 32 bits).
fn parse_ip(hex: &str) -> Option<IpAddr> {
    match hex.len() {
        8 => {
            let n = u32::from_str_radix(hex, 16).ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(n.swap_bytes())))
        }
        32 => {
            let mut bytes = [0u8; 16];
            for i in 0..4 {
                let palavra = u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).ok()?;
                bytes[i * 4..i * 4 + 4].copy_from_slice(&palavra.to_le_bytes());
            }
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// Lê as linhas em LISTEN de um arquivo no formato do `/proc/net/tcp`.
fn parse_tabela(conteudo: &str) -> Vec<Socket> {
    conteudo
        .lines()
        .skip(1)
        .filter_map(|linha| {
            let c: Vec<&str> = linha.split_whitespace().collect();
            if c.len() < 10 || c[3] != TCP_LISTEN {
                return None;
            }
            let (ip_hex, porta_hex) = c[1].split_once(':')?;
            Some(Socket {
                ip: parse_ip(ip_hex)?,
                porta: u16::from_str_radix(porta_hex, 16).ok()?,
                uid: c[7].parse().ok()?,
                inode: c[9].parse().ok()?,
            })
        })
        .collect()
}

fn ler_sockets() -> Vec<Socket> {
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .flat_map(|c| parse_tabela(&c))
        .collect()
}

/// Varre `/proc/<pid>/fd` procurando os inodes pedidos. Só os processos legíveis
/// (do próprio usuário) entram; os demais são ignorados silenciosamente.
fn mapear_inodes(alvos: &HashSet<u64>) -> HashMap<u64, u32> {
    let mut mapa = HashMap::new();
    if alvos.is_empty() {
        return mapa;
    }
    let Ok(procs) = fs::read_dir("/proc") else {
        return mapa;
    };
    for entrada in procs.flatten() {
        let Some(pid) = entrada.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(fds) = fs::read_dir(entrada.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(destino) = fs::read_link(fd.path()) else {
                continue;
            };
            let destino = destino.to_string_lossy();
            if let Some(inode) = destino
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.parse::<u64>().ok())
            {
                if alvos.contains(&inode) {
                    mapa.entry(inode).or_insert(pid);
                }
            }
        }
        if mapa.len() == alvos.len() {
            break;
        }
    }
    mapa
}

pub fn home() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
}

pub fn com_til(caminho: &str) -> String {
    match home() {
        Some(h) if caminho == h => "~".into(),
        Some(h) if caminho.starts_with(&format!("{h}/")) => format!("~{}", &caminho[h.len()..]),
        _ => caminho.into(),
    }
}

fn resumir_cmdline(bruto: &[u8]) -> String {
    let partes: Vec<String> = bruto
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    let mut partes = partes.into_iter();
    let Some(primeiro) = partes.next() else {
        return String::new();
    };
    // Mostra só o nome do executável, não o caminho inteiro.
    let exe = primeiro.rsplit('/').next().unwrap_or(&primeiro).to_string();
    let resto: Vec<String> = partes.map(|p| com_til(&p)).collect();
    let mut texto = std::iter::once(exe).chain(resto).collect::<Vec<_>>().join(" ");
    if texto.chars().count() > TAM_MAX_CMDLINE {
        texto = texto.chars().take(TAM_MAX_CMDLINE - 1).collect::<String>() + "…";
    }
    texto
}

fn ler_processo(pid: u32) -> Option<Processo> {
    let base = format!("/proc/{pid}");
    let comm = fs::read_to_string(format!("{base}/comm")).ok()?;
    // O node renomeia a thread principal para "node-MainThread".
    let programa = comm.trim().trim_end_matches("-MainThread").to_string();
    let executavel = fs::read_link(format!("{base}/exe"))
        .ok()
        .and_then(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_string())
        });
    let cmdline = fs::read(format!("{base}/cmdline"))
        .map(|b| resumir_cmdline(&b))
        .unwrap_or_default();
    let pasta = fs::read_link(format!("{base}/cwd"))
        .ok()
        .map(|p| com_til(&p.to_string_lossy()));
    Some(Processo { pid, programa, executavel, cmdline, pasta })
}

fn meu_uid() -> Option<u32> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|l| l.split_whitespace().next())
        .and_then(|s| s.parse().ok())
}

/// Lista as portas TCP em escuta agora, ordenadas por número.
pub fn listar_portas() -> Vec<Porta> {
    let sockets = ler_sockets();
    let alvos: HashSet<u64> = sockets.iter().map(|s| s.inode).filter(|i| *i != 0).collect();
    let inode_pid = mapear_inodes(&alvos);
    let uid = meu_uid();
    let mut cache_proc: HashMap<u32, Option<Processo>> = HashMap::new();

    let mut por_porta: BTreeMap<u16, Porta> = BTreeMap::new();
    for s in &sockets {
        let processo = inode_pid
            .get(&s.inode)
            .and_then(|pid| cache_proc.entry(*pid).or_insert_with(|| ler_processo(*pid)).clone());
        let endereco = formatar_ip(&s.ip);
        let escopo = escopo_do_ip(&s.ip);
        let outro = uid.is_some_and(|u| u != s.uid);
        let p = por_porta.entry(s.porta).or_insert_with(|| Porta {
            porta: s.porta,
            enderecos: Vec::new(),
            escopo,
            processo: None,
            outro_usuario: outro,
        });
        if !p.enderecos.contains(&endereco) {
            p.enderecos.push(endereco);
        }
        if escopo == Escopo::Rede {
            p.escopo = Escopo::Rede;
        }
        if p.processo.is_none() {
            p.processo = processo;
        }
        p.outro_usuario = p.outro_usuario && outro;
    }
    por_porta.into_values().collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn ipv4_little_endian() {
        assert_eq!(parse_ip("0100007F"), Some("127.0.0.1".parse().unwrap()));
        assert_eq!(parse_ip("00000000"), Some("0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn ipv6() {
        assert_eq!(
            parse_ip("00000000000000000000000001000000"),
            Some("::1".parse().unwrap())
        );
        assert_eq!(
            parse_ip("0000000000000000FFFF00000100007F"),
            Some("::ffff:127.0.0.1".parse().unwrap())
        );
    }

    #[test]
    fn escopos() {
        assert_eq!(escopo_do_ip(&"127.0.0.53".parse().unwrap()), Escopo::Local);
        assert_eq!(escopo_do_ip(&"::1".parse().unwrap()), Escopo::Local);
        assert_eq!(escopo_do_ip(&"::ffff:127.0.0.1".parse().unwrap()), Escopo::Local);
        assert_eq!(escopo_do_ip(&"0.0.0.0".parse().unwrap()), Escopo::Rede);
        assert_eq!(escopo_do_ip(&"::".parse().unwrap()), Escopo::Rede);
        assert_eq!(escopo_do_ip(&"192.168.0.10".parse().unwrap()), Escopo::Rede);
    }

    #[test]
    fn tabela_so_listen() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1 0000000000000000 100 0 0 10 0\n\
   1: 0100007F:1F91 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 12346 1 0000000000000000 100 0 0 10 0\n";
        let s = parse_tabela(t);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].porta, 8080);
        assert_eq!(s[0].inode, 12345);
        assert_eq!(s[0].uid, 1000);
    }

    #[test]
    fn cmdline_resumida() {
        assert_eq!(
            resumir_cmdline(b"/usr/bin/python3\0-m\0http.server\08765\0"),
            "python3 -m http.server 8765"
        );
    }
}
