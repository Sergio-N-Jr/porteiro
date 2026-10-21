//! Configuração persistida em `~/.config/porteiro/config.json`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Monitorado {
    pub id: String,
    pub nome: String,
    pub porta: u16,
    /// Caminho opcional da URL, ex.: `/admin`.
    pub caminho: String,
    /// `http` ou `https`.
    pub protocolo: String,
    pub observacao: String,
    pub notificar: bool,
}

impl Default for Monitorado {
    fn default() -> Self {
        Self {
            id: String::new(),
            nome: String::new(),
            porta: 0,
            caminho: String::new(),
            protocolo: "http".into(),
            observacao: String::new(),
            notificar: true,
        }
    }
}

impl Monitorado {
    fn exemplo(id: &str, nome: &str, porta: u16) -> Self {
        Self { id: id.into(), nome: nome.into(), porta, ..Default::default() }
    }

    pub fn url_local(&self) -> String {
        format!("{}://localhost:{}{}", self.protocolo_ok(), self.porta, self.caminho_ok())
    }

    pub fn protocolo_ok(&self) -> &str {
        if self.protocolo == "https" { "https" } else { "http" }
    }

    pub fn caminho_ok(&self) -> String {
        let c = self.caminho.trim();
        if c.is_empty() || c.starts_with('/') { c.into() } else { format!("/{c}") }
    }
}

/// Regra de ocultação: casa por programa (exato, sem diferenciar maiúsculas) e/ou porta.
/// Se as duas partes estiverem preenchidas, as duas precisam casar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Ignorado {
    pub programa: Option<String>,
    pub porta: Option<u16>,
}

impl Ignorado {
    fn programa(p: &str) -> Self {
        Self { programa: Some(p.into()), porta: None }
    }

    /// `nomes`: nomes do processo (programa e executável); vazio se desconhecido.
    pub fn casa(&self, nomes: &[&str], porta: u16) -> bool {
        let prog = self.programa.as_deref().map(str::trim).filter(|p| !p.is_empty());
        if prog.is_none() && self.porta.is_none() {
            return false;
        }
        let ok_prog = match prog {
            Some(p) => nomes.iter().any(|x| x.eq_ignore_ascii_case(p)),
            None => true,
        };
        let ok_porta = self.porta.is_none_or(|p| p == porta);
        ok_prog && ok_porta
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub monitorados: Vec<Monitorado>,
    pub ignorados: Vec<Ignorado>,
    /// Intervalo entre leituras, em milissegundos.
    pub intervalo_ms: u64,
    /// Liga/desliga todas as notificações.
    pub notificacoes: bool,
    /// Nome extra publicado via mDNS como `<nome_rede>.local` (vazio = desligado).
    pub nome_rede: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            monitorados: Vec::new(),
            ignorados: Self::ignorados_padrao(),
            intervalo_ms: 2000,
            notificacoes: true,
            nome_rede: String::new(),
        }
    }
}

impl Config {
    pub fn ignorados_padrao() -> Vec<Ignorado> {
        [
            "steam",
            "Discord",
            "code",
            "language_server",
            "adb",
            "systemd-resolve",
            "cupsd",
            "kdeconnectd",
            "avahi-daemon",
            "dnsmasq",
            "sshd",
            "spotify",
            "slack",
        ]
        .iter()
        .map(|p| Ignorado::programa(p))
        .chain([
            // Portas de sistema que costumam aparecer sem programa (root).
            Ignorado { programa: None, porta: Some(53) },
            Ignorado { programa: None, porta: Some(631) },
            Ignorado { programa: None, porta: Some(5355) },
        ])
        .collect()
    }

    /// Primeira configuração: ignorados padrão e os painéis do Maestro como exemplo.
    fn inicial() -> Self {
        Self {
            monitorados: vec![
                Monitorado::exemplo("painel-app-aluno", "Painel Maestro App Aluno", 4747),
                Monitorado::exemplo("painel-app-pro", "Painel Maestro App Pro", 4748),
            ],
            ..Default::default()
        }
    }

    pub fn intervalo(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.intervalo_ms.clamp(500, 60_000))
    }

    pub fn ignorado(&self, nomes: &[&str], porta: u16) -> bool {
        self.ignorados.iter().any(|r| r.casa(nomes, porta))
    }
}

fn base_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| crate::scan::home().map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn caminho() -> PathBuf {
    base_config().join("porteiro").join("config.json")
}

/// Onde a config ficava quando o app se chamava Localhosts.
fn caminho_antigo() -> PathBuf {
    base_config().join("localhosts").join("config.json")
}

/// Carrega a configuração; se o arquivo não existir, migra a do Localhosts ou cria com os valores iniciais.
pub fn carregar() -> Config {
    let p = caminho();
    match fs::read_to_string(&p) {
        Ok(texto) => serde_json::from_str(&texto).unwrap_or_else(|e| {
            eprintln!("[porteiro] config inválida em {}: {e}; usando padrão em memória", p.display());
            Config::default()
        }),
        Err(_) => {
            let c = fs::read_to_string(caminho_antigo())
                .ok()
                .and_then(|texto| serde_json::from_str(&texto).ok())
                .unwrap_or_else(Config::inicial);
            if let Err(e) = salvar(&c) {
                eprintln!("[porteiro] não consegui criar {}: {e}", p.display());
            }
            c
        }
    }
}

/// Grava de forma atômica (arquivo temporário + rename).
pub fn salvar(c: &Config) -> std::io::Result<()> {
    let p = caminho();
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(c)?)?;
    fs::rename(tmp, p)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn regras() {
        let so_prog = Ignorado::programa("steam");
        assert!(so_prog.casa(&["Steam"], 1));
        assert!(!so_prog.casa(&[], 1));
        let so_porta = Ignorado { programa: None, porta: Some(53) };
        assert!(so_porta.casa(&[], 53));
        assert!(!so_porta.casa(&["x"], 54));
        let ambos = Ignorado { programa: Some("node".into()), porta: Some(3000) };
        assert!(ambos.casa(&["node"], 3000));
        assert!(!ambos.casa(&["node"], 3001));
        assert!(!Ignorado::default().casa(&["x"], 1));
    }

    #[test]
    fn url() {
        let m = Monitorado { porta: 3000, caminho: "admin".into(), ..Default::default() };
        assert_eq!(m.url_local(), "http://localhost:3000/admin");
    }
}
