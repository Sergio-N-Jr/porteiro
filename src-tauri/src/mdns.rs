//! Publica um nome extra `<nome>.local` → IP da LAN via mDNS, com `avahi-publish -a -R`.
//!
//! Não mexe no hostname nem em nada do sistema: o nome existe só enquanto o processo
//! filho `avahi-publish` vive, e ele morre junto com o app (PR_SET_PDEATHSIG + kill no Sair).

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Depois de uma falha, tenta de novo no máximo a cada este intervalo.
const RETENTAR_APOS: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Situacao {
    #[default]
    Desligado,
    Publicando,
    Publicado,
    Erro,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct StatusNome {
    /// Nome completo, ex.: `painel-app.local` (vazio se desligado).
    pub nome: String,
    /// IP publicado (ou que se tentou publicar).
    pub ip: Option<String>,
    pub situacao: Situacao,
    pub mensagem: Option<String>,
}

/// Valida o rótulo DNS: minúsculas, números e hífen; 1–63; sem hífen nas pontas.
pub fn nome_valido(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 63
        && !n.starts_with('-')
        && !n.ends_with('-')
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Interpreta uma linha de saída do avahi-publish (rodando com `LC_ALL=C`).
fn interpretar(linha: &str, esperado: &str) -> Option<(Situacao, Option<String>)> {
    if let Some(resto) = linha.strip_prefix("Established under name '") {
        let nome = resto.trim_end().trim_end_matches('\'');
        return Some(if nome.eq_ignore_ascii_case(esperado) {
            (Situacao::Publicado, None)
        } else {
            (Situacao::Erro, Some(format!("{esperado} já está em uso na rede")))
        });
    }
    if linha.contains("collision") || linha.contains("Collision") {
        return Some((Situacao::Erro, Some(format!("{esperado} já está em uso na rede"))));
    }
    if linha.starts_with("Failed") || linha.contains("failed") {
        return Some((Situacao::Erro, Some(format!("avahi-publish: {}", linha.trim()))));
    }
    None
}

enum Pedido {
    Garantir { nome: Option<String>, ip: Option<String> },
}

/// Controla o processo `avahi-publish` numa thread própria (que vive o processo inteiro,
/// para o PR_SET_PDEATHSIG não disparar à toa).
pub struct Publicador {
    tx: Sender<Pedido>,
    status: Arc<Mutex<StatusNome>>,
    pid: Arc<AtomicU32>,
}

impl Publicador {
    /// `ao_mudar` é chamado (de outra thread) sempre que o status muda.
    pub fn iniciar(ao_mudar: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Pedido>();
        let status = Arc::new(Mutex::new(StatusNome::default()));
        let pid = Arc::new(AtomicU32::new(0));
        let ao_mudar: Arc<dyn Fn() + Send + Sync> = Arc::new(ao_mudar);
        let (st, pid2) = (status.clone(), pid.clone());
        std::thread::Builder::new()
            .name("mdns".into())
            .spawn(move || {
                let mut ctl = Controle {
                    filho: Arc::new(Mutex::new(None)),
                    atual: None,
                    ultima_tentativa: None,
                    geracao: Arc::new(AtomicU64::new(0)),
                    status: st,
                    pid: pid2,
                    ao_mudar,
                };
                let mut desejado: (Option<String>, Option<String>) = (None, None);
                loop {
                    match rx.recv_timeout(RETENTAR_APOS) {
                        Ok(Pedido::Garantir { nome, ip }) => desejado = (nome, ip),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    ctl.garantir(desejado.0.as_deref(), desejado.1.as_deref());
                }
                ctl.parar();
            })
            .expect("thread mdns");
        Self { tx, status, pid }
    }

    /// Pede o nome (sem `.local`) publicado no IP dado. `None` em qualquer um desliga.
    pub fn garantir(&self, nome: Option<String>, ip: Option<String>) {
        let _ = self.tx.send(Pedido::Garantir { nome, ip });
    }

    pub fn status(&self) -> StatusNome {
        self.status.lock().unwrap().clone()
    }

    /// Encerra o avahi-publish imediatamente (chamado ao sair do app).
    pub fn encerrar_ja(&self) {
        let pid = self.pid.swap(0, Ordering::SeqCst);
        if pid != 0 {
            // SAFETY: kill(2) só envia um sinal; o pid é o do nosso filho.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
    }
}

struct Controle {
    filho: Arc<Mutex<Option<Child>>>,
    /// (nome completo, ip) que está publicado/tentando agora.
    atual: Option<(String, String)>,
    ultima_tentativa: Option<Instant>,
    /// Incrementa a cada processo novo; leitores antigos ignoram as próprias linhas.
    geracao: Arc<AtomicU64>,
    status: Arc<Mutex<StatusNome>>,
    pid: Arc<AtomicU32>,
    ao_mudar: Arc<dyn Fn() + Send + Sync>,
}

impl Controle {
    fn definir(&self, novo: StatusNome) {
        let mudou = {
            let mut s = self.status.lock().unwrap();
            let mudou = *s != novo;
            *s = novo;
            mudou
        };
        if mudou {
            (self.ao_mudar)();
        }
    }

    fn parar(&mut self) {
        self.geracao.fetch_add(1, Ordering::SeqCst);
        self.pid.store(0, Ordering::SeqCst);
        if let Some(mut c) = self.filho.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.atual = None;
    }

    fn vivo(&self) -> bool {
        self.filho
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    fn garantir(&mut self, nome: Option<&str>, ip: Option<&str>) {
        let Some(nome) = nome.filter(|n| !n.is_empty()) else {
            self.parar();
            self.definir(StatusNome::default());
            return;
        };
        let completo = format!("{nome}.local");
        let Some(ip) = ip else {
            self.parar();
            self.definir(StatusNome {
                nome: completo,
                ip: None,
                situacao: Situacao::Erro,
                mensagem: Some("Sem IP de rede local (cabo/Wi-Fi desconectado?)".into()),
            });
            return;
        };
        let alvo = (completo.clone(), ip.to_string());
        if self.atual.as_ref() == Some(&alvo) {
            let falhou = self.status.lock().unwrap().situacao == Situacao::Erro || !self.vivo();
            let pode_retentar = self.ultima_tentativa.is_none_or(|t| t.elapsed() >= RETENTAR_APOS);
            if !(falhou && pode_retentar) {
                return;
            }
        }
        self.parar();
        self.publicar(alvo);
    }

    fn publicar(&mut self, (completo, ip): (String, String)) {
        self.ultima_tentativa = Some(Instant::now());
        self.atual = Some((completo.clone(), ip.clone()));
        let base = StatusNome {
            nome: completo.clone(),
            ip: Some(ip.clone()),
            situacao: Situacao::Publicando,
            mensagem: None,
        };
        self.definir(base.clone());

        let mut cmd = Command::new("avahi-publish");
        cmd.args(["-a", "-R", &completo, &ip])
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: só chama prctl (async-signal-safe) entre o fork e o exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut filho = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    "avahi-publish não encontrado (instale o pacote avahi)".to_string()
                } else {
                    format!("Não consegui iniciar o avahi-publish: {e}")
                };
                eprintln!("[porteiro] mdns: {msg}");
                self.definir(StatusNome { situacao: Situacao::Erro, mensagem: Some(msg), ..base });
                return;
            }
        };
        eprintln!("[porteiro] mdns: publicando {completo} → {ip} (pid {})", filho.id());
        self.pid.store(filho.id(), Ordering::SeqCst);
        let geracao = self.geracao.fetch_add(1, Ordering::SeqCst) + 1;

        for leitor in [
            filho.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            filho.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let (status, ao_mudar, ger, filho_arc) =
                (self.status.clone(), self.ao_mudar.clone(), self.geracao.clone(), self.filho.clone());
            let (base, completo) = (base.clone(), completo.clone());
            std::thread::spawn(move || {
                let mut ultima = String::new();
                for linha in BufReader::new(leitor).lines().map_while(Result::ok) {
                    if ger.load(Ordering::SeqCst) != geracao {
                        return;
                    }
                    eprintln!("[porteiro] avahi-publish: {linha}");
                    ultima = linha.clone();
                    if let Some((situacao, mensagem)) = interpretar(&linha, &completo) {
                        if situacao == Situacao::Erro {
                            // Colisão: o avahi trocaria para "nome-2.local"; melhor parar.
                            if let Some(c) = filho_arc.lock().unwrap().as_mut() {
                                let _ = c.kill();
                            }
                        }
                        *status.lock().unwrap() = StatusNome { situacao, mensagem, ..base.clone() };
                        ao_mudar();
                    }
                }
                // Fim da saída: o processo terminou. Se ainda era o atual e não houve erro, registra.
                if ger.load(Ordering::SeqCst) == geracao {
                    let mut s = status.lock().unwrap();
                    if s.situacao != Situacao::Erro && !ultima.is_empty() {
                        *s = StatusNome {
                            situacao: Situacao::Erro,
                            mensagem: Some(format!("avahi-publish terminou: {ultima}")),
                            ..base
                        };
                        drop(s);
                        ao_mudar();
                    }
                }
            });
        }
        *self.filho.lock().unwrap() = Some(filho);
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn validacao() {
        assert!(nome_valido("painel-app"));
        assert!(nome_valido("a1"));
        assert!(!nome_valido(""));
        assert!(!nome_valido("Painel"));
        assert!(!nome_valido("-x"));
        assert!(!nome_valido("x-"));
        assert!(!nome_valido("a.b"));
        assert!(!nome_valido(&"a".repeat(64)));
    }

    #[test]
    fn saida_do_avahi() {
        let e = "painel-app.local";
        assert_eq!(
            interpretar("Established under name 'painel-app.local'", e),
            Some((Situacao::Publicado, None))
        );
        assert_eq!(
            interpretar("Established under name 'painel-app-2.local'", e).map(|x| x.0),
            Some(Situacao::Erro)
        );
        assert_eq!(
            interpretar("Name collision, picking new name 'painel-app-2.local'.", e),
            Some((Situacao::Erro, Some("painel-app.local já está em uso na rede".into())))
        );
        assert_eq!(interpretar("Failed to add address: Local name collision", e).map(|x| x.0), Some(Situacao::Erro));
        assert_eq!(interpretar("Got SIGTERM, quitting.", e), None);
    }
}
