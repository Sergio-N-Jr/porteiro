mod config;
mod mdns;
mod rede;
mod scan;

use config::{Config, Monitorado};
use rede::InfoRede;
use scan::{Escopo, Porta, Processo};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent, Wry};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Notify;

const ID_TRAY: &str = "principal";

/// Uma porta descoberta, já marcada contra a configuração.
#[derive(Debug, Clone, Serialize)]
pub struct Descoberto {
    #[serde(flatten)]
    pub porta: Porta,
    pub ignorado: bool,
    /// Id do monitorado que usa esta porta, se houver.
    pub monitorado: Option<String>,
}

/// Situação atual de um projeto monitorado.
#[derive(Debug, Clone, Serialize)]
pub struct StatusMonitorado {
    pub id: String,
    pub online: bool,
    pub escopo: Option<Escopo>,
    pub enderecos: Vec<String>,
    pub processo: Option<Processo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Estado {
    pub descobertos: Vec<Descoberto>,
    pub status: Vec<StatusMonitorado>,
    pub rede: InfoRede,
    /// Nome extra publicado via mDNS (só no app; `None` no `--dump`).
    pub nome_rede: Option<mdns::StatusNome>,
    pub atualizado_em: u64,
}

impl Estado {
    fn online(&self) -> usize {
        self.status.iter().filter(|s| s.online).count()
    }
}

/// Monta o estado a partir das portas lidas e da configuração (função pura, testável).
pub fn montar_estado(cfg: &Config, portas: Vec<Porta>, rede: InfoRede) -> Estado {
    let status = cfg
        .monitorados
        .iter()
        .map(|m| {
            let p = portas.iter().find(|p| p.porta == m.porta);
            StatusMonitorado {
                id: m.id.clone(),
                online: p.is_some(),
                escopo: p.map(|p| p.escopo),
                enderecos: p.map(|p| p.enderecos.clone()).unwrap_or_default(),
                processo: p.and_then(|p| p.processo.clone()),
            }
        })
        .collect();
    let descobertos = portas
        .into_iter()
        .map(|p| {
            let nomes: Vec<&str> = p
                .processo
                .iter()
                .flat_map(|x| std::iter::once(x.programa.as_str()).chain(x.executavel.as_deref()))
                .collect();
            Descoberto {
                ignorado: cfg.ignorado(&nomes, p.porta),
                monitorado: cfg.monitorados.iter().find(|m| m.porta == p.porta).map(|m| m.id.clone()),
                porta: p,
            }
        })
        .collect();
    let atualizado_em = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    Estado { descobertos, status, rede, nome_rede: None, atualizado_em }
}

/// Lê tudo do sistema (bloqueante: varre o /proc).
pub fn ler_estado(cfg: &Config) -> Estado {
    montar_estado(cfg, scan::listar_portas(), rede::info())
}

/// `--dump`: imprime o estado atual em JSON e sai (sem abrir janela).
pub fn dump() {
    let cfg = config::carregar();
    let estado = ler_estado(&cfg);
    match serde_json::to_string_pretty(&estado) {
        Ok(j) => println!("{j}"),
        Err(e) => eprintln!("erro: {e}"),
    }
}

#[derive(Default)]
struct Interno {
    config: Config,
    ultimo: Option<Estado>,
    /// Última situação conhecida de cada monitorado, por `id:porta`.
    anteriores: HashMap<String, bool>,
    assinatura_menu: String,
}

struct Compartilhado {
    interno: Mutex<Interno>,
    acordar: Notify,
    mdns: mdns::Publicador,
}

type Shared = Arc<Compartilhado>;

fn chave(m: &Monitorado) -> String {
    format!("{}:{}", m.id, m.porta)
}

fn notificar(app: &AppHandle, titulo: &str, corpo: &str) {
    eprintln!("[porteiro] notificação: {titulo} — {corpo}");
    if let Err(e) = app.notification().builder().title(titulo).body(corpo).show() {
        eprintln!("[porteiro] falha ao notificar: {e}");
    }
}

fn texto_resumo(online: usize, total: usize) -> String {
    format!("Porteiro — {online} de {total} online")
}

fn montar_menu(app: &AppHandle, cfg: &Config, estado: Option<&Estado>) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    let online_de = |id: &str| estado.and_then(|e| e.status.iter().find(|s| s.id == id)).is_some_and(|s| s.online);
    let total = cfg.monitorados.len();
    let n_online = cfg.monitorados.iter().filter(|m| online_de(&m.id)).count();
    menu.append(&MenuItem::with_id(app, "resumo", texto_resumo(n_online, total), false, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if cfg.monitorados.is_empty() {
        menu.append(&MenuItem::with_id(app, "vazio", "Nenhum projeto monitorado", false, None::<&str>)?)?;
    }
    for m in &cfg.monitorados {
        let bolinha = if online_de(&m.id) { "●" } else { "○" };
        let texto = format!("{bolinha}  {}  (:{})", m.nome, m.porta);
        menu.append(&MenuItem::with_id(app, format!("abrir:{}", m.id), texto, true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "mostrar", "Mostrar janela", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "sair", "Sair", true, None::<&str>)?)?;
    Ok(menu)
}

fn assinatura(cfg: &Config, estado: &Estado) -> String {
    cfg.monitorados
        .iter()
        .map(|m| {
            let on = estado.status.iter().any(|s| s.id == m.id && s.online);
            format!("{}|{}|{}|{on}", m.id, m.nome, m.porta)
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn mostrar_janela(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Aplica um estado novo: notificações de transição, bandeja e evento para a UI.
fn aplicar(app: &AppHandle, shared: &Shared, mut estado: Estado) {
    let nome = shared.interno.lock().unwrap().config.nome_rede.trim().to_string();
    let nome = Some(nome).filter(|n| mdns::nome_valido(n));
    shared.mdns.garantir(nome, estado.rede.ip_lan.clone());
    estado.nome_rede = Some(shared.mdns.status());
    let mut avisos: Vec<(String, String)> = Vec::new();
    let mut menu_novo = None;
    {
        let mut g = shared.interno.lock().unwrap();
        let cfg = g.config.clone();
        let mut novos = HashMap::new();
        for m in &cfg.monitorados {
            let online = estado.status.iter().any(|s| s.id == m.id && s.online);
            let k = chave(m);
            if let Some(&antes) = g.anteriores.get(&k) {
                if antes != online && cfg.notificacoes && m.notificar {
                    avisos.push(if online {
                        (format!("{} voltou", m.nome), format!("Online de novo na porta {}", m.porta))
                    } else {
                        (format!("{} caiu (porta {})", m.nome, m.porta), "Ninguém escutando nessa porta agora".into())
                    });
                }
            }
            novos.insert(k, online);
        }
        g.anteriores = novos;
        let ass = assinatura(&cfg, &estado);
        if ass != g.assinatura_menu {
            g.assinatura_menu = ass;
            menu_novo = Some(cfg);
        }
        g.ultimo = Some(estado.clone());
    }
    for (t, c) in avisos {
        notificar(app, &t, &c);
    }
    if let Some(cfg) = menu_novo {
        atualizar_bandeja(app, &cfg, Some(&estado));
    }
    let _ = app.emit("estado", &estado);
}

fn atualizar_bandeja(app: &AppHandle, cfg: &Config, estado: Option<&Estado>) {
    let Some(tray) = app.tray_by_id(ID_TRAY) else { return };
    let online = estado.map(|e| e.online()).unwrap_or(0);
    let _ = tray.set_tooltip(Some(texto_resumo(online, cfg.monitorados.len())));
    match montar_menu(app, cfg, estado) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => eprintln!("[porteiro] erro ao montar menu: {e}"),
    }
}

fn iniciar_laco(app: AppHandle, shared: Shared) {
    tauri::async_runtime::spawn(async move {
        loop {
            let cfg = shared.interno.lock().unwrap().config.clone();
            let intervalo = cfg.intervalo();
            match tauri::async_runtime::spawn_blocking(move || ler_estado(&cfg)).await {
                Ok(estado) => aplicar(&app, &shared, estado),
                Err(e) => eprintln!("[porteiro] erro na leitura: {e}"),
            }
            tokio::select! {
                _ = tokio::time::sleep(intervalo) => {}
                _ = shared.acordar.notified() => {}
            }
        }
    });
}

// ---------- comandos ----------

#[tauri::command]
fn obter_estado(shared: tauri::State<'_, Shared>) -> Option<Estado> {
    shared.interno.lock().unwrap().ultimo.clone()
}

#[tauri::command]
fn obter_config(shared: tauri::State<'_, Shared>) -> Config {
    shared.interno.lock().unwrap().config.clone()
}

#[tauri::command]
fn salvar_config(app: AppHandle, shared: tauri::State<'_, Shared>, mut config: Config) -> Result<Config, String> {
    config.nome_rede = config.nome_rede.trim().to_string();
    if !config.nome_rede.is_empty() && !mdns::nome_valido(&config.nome_rede) {
        return Err("Nome na rede inválido: use só letras minúsculas, números e hífen (sem hífen no começo ou no fim).".into());
    }
    config::salvar(&config).map_err(|e| format!("Não consegui gravar {}: {e}", config::caminho().display()))?;
    let ultimo = {
        let mut g = shared.interno.lock().unwrap();
        g.config = config.clone();
        g.assinatura_menu.clear();
        g.ultimo.clone()
    };
    atualizar_bandeja(&app, &config, ultimo.as_ref());
    shared.acordar.notify_one();
    Ok(config)
}

#[tauri::command]
fn caminho_config() -> String {
    scan::com_til(&config::caminho().to_string_lossy())
}

#[tauri::command]
fn ignorados_padrao() -> Vec<config::Ignorado> {
    Config::ignorados_padrao()
}

/// Lista as portas em escuta sob demanda, sem travar a UI.
#[tauri::command]
async fn listar_portas() -> Result<Vec<Porta>, String> {
    tauri::async_runtime::spawn_blocking(scan::listar_portas)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn atualizar_agora(shared: tauri::State<'_, Shared>) {
    shared.acordar.notify_one();
}

#[tauri::command]
fn testar_notificacao(app: AppHandle) {
    notificar(&app, "Porteiro", "As notificações estão funcionando.");
}

/// Tela pedida pela linha de comando (`--adicionar` ou `--config`).
fn tela_dos_args<I: IntoIterator<Item = String>>(args: I) -> Option<&'static str> {
    args.into_iter().find_map(|a| match a.as_str() {
        "--adicionar" => Some("form"),
        "--config" | "--configuracoes" => Some("config"),
        _ => None,
    })
}

#[tauri::command]
fn tela_inicial() -> Option<&'static str> {
    tela_dos_args(std::env::args())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let oculto = std::env::args().any(|a| a == "--oculto" || a == "--minimizado");
    let shared: Shared = Arc::new_cyclic(|fraco: &std::sync::Weak<Compartilhado>| {
        let fraco = fraco.clone();
        Compartilhado {
            interno: Mutex::new(Interno { config: config::carregar(), ..Default::default() }),
            acordar: Notify::new(),
            // Quando o status do nome muda, acorda o laço para a UI ver logo.
            mdns: mdns::Publicador::iniciar(move || {
                if let Some(s) = fraco.upgrade() {
                    s.acordar.notify_one();
                }
            }),
        }
    });
    let shared_saida = shared.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            mostrar_janela(app);
            if let Some(tela) = tela_dos_args(args) {
                let _ = app.emit("ir-para", tela);
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            obter_estado,
            obter_config,
            salvar_config,
            caminho_config,
            ignorados_padrao,
            listar_portas,
            atualizar_agora,
            testar_notificacao,
            tela_inicial
        ])
        .on_window_event(|janela, evento| {
            // Fechar a janela só esconde: o app continua na bandeja.
            if let WindowEvent::CloseRequested { api, .. } = evento {
                api.prevent_close();
                let _ = janela.hide();
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let cfg = shared.interno.lock().unwrap().config.clone();
            let menu = montar_menu(&handle, &cfg, None)?;
            let mut tray = TrayIconBuilder::with_id(ID_TRAY)
                .tooltip(texto_resumo(0, cfg.monitorados.len()))
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, ev| {
                    let id = ev.id().as_ref();
                    match id {
                        "mostrar" => mostrar_janela(app),
                        "sair" => app.exit(0),
                        _ => {
                            if let Some(mid) = id.strip_prefix("abrir:") {
                                let st: tauri::State<Shared> = app.state();
                                let m = st.interno.lock().unwrap().config.monitorados.iter().find(|m| m.id == mid).cloned();
                                if let Some(m) = m {
                                    if let Err(e) = app.opener().open_url(m.url_local(), None::<&str>) {
                                        eprintln!("[porteiro] erro ao abrir {}: {e}", m.url_local());
                                    }
                                }
                            }
                        }
                    }
                });
            if let Some(icone) = app.default_window_icon() {
                tray = tray.icon(icone.clone());
            }
            tray.build(app)?;

            if !oculto {
                mostrar_janela(&handle);
            }
            iniciar_laco(handle, shared.clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o Porteiro")
        .run(move |_app, evento| {
            // "Sair" da bandeja (ou fim normal): derruba o avahi-publish na hora.
            if let RunEvent::Exit = evento {
                shared_saida.mdns.encerrar_ja();
            }
        });
}

#[cfg(test)]
mod testes {
    use super::*;

    fn porta(n: u16, escopo: Escopo, programa: &str) -> Porta {
        Porta {
            porta: n,
            enderecos: vec![if escopo == Escopo::Local { "127.0.0.1" } else { "0.0.0.0" }.into()],
            escopo,
            processo: Some(Processo { pid: 1, programa: programa.into(), executavel: None, cmdline: programa.into(), pasta: None }),
            outro_usuario: false,
        }
    }

    #[test]
    fn estado_marca_monitorados_e_ignorados() {
        let cfg = Config {
            monitorados: vec![Monitorado { id: "a".into(), nome: "A".into(), porta: 3000, ..Default::default() },
                Monitorado { id: "b".into(), nome: "B".into(), porta: 3001, ..Default::default() }],
            ..Default::default()
        };
        let e = montar_estado(
            &cfg,
            vec![porta(3000, Escopo::Local, "node"), porta(44261, Escopo::Local, "steam")],
            InfoRede { ip_lan: None, interface: None, hostname: "x".into() },
        );
        assert!(e.status[0].online);
        assert_eq!(e.status[0].escopo, Some(Escopo::Local));
        assert!(!e.status[1].online);
        assert_eq!(e.descobertos[0].monitorado.as_deref(), Some("a"));
        assert!(e.descobertos[1].ignorado);
    }

    /// Leitura real do sistema: `cargo test -- --ignored --nocapture leitura_real`.
    #[test]
    #[ignore]
    fn leitura_real() {
        let portas = scan::listar_portas();
        for p in &portas {
            println!("{:>5} {:?} {:?} {:?}", p.porta, p.escopo, p.enderecos, p.processo.as_ref().map(|x| (&x.programa, &x.pasta)));
        }
        assert!(!portas.is_empty());
    }
}
