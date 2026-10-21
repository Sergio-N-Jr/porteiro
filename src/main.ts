import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

// ---------- tipos (espelham o Rust) ----------

type Escopo = "local" | "rede";

interface Processo {
  pid: number;
  programa: string;
  executavel: string | null;
  cmdline: string;
  pasta: string | null;
}

interface Descoberto {
  porta: number;
  enderecos: string[];
  escopo: Escopo;
  processo: Processo | null;
  outro_usuario: boolean;
  ignorado: boolean;
  monitorado: string | null;
}

interface StatusMonitorado {
  id: string;
  online: boolean;
  escopo: Escopo | null;
  enderecos: string[];
  processo: Processo | null;
}

interface InfoRede {
  ip_lan: string | null;
  interface: string | null;
  hostname: string;
}

interface Estado {
  descobertos: Descoberto[];
  status: StatusMonitorado[];
  rede: InfoRede;
  nome_rede: StatusNome | null;
  atualizado_em: number;
}

interface StatusNome {
  nome: string;
  ip: string | null;
  situacao: "desligado" | "publicando" | "publicado" | "erro";
  mensagem: string | null;
}

interface Monitorado {
  id: string;
  nome: string;
  porta: number;
  caminho: string;
  protocolo: "http" | "https";
  observacao: string;
  notificar: boolean;
}

interface Ignorado {
  programa: string | null;
  porta: number | null;
}

interface Config {
  monitorados: Monitorado[];
  ignorados: Ignorado[];
  intervalo_ms: number;
  notificacoes: boolean;
  nome_rede: string;
}

type Tela = "principal" | "form" | "config";

// ---------- estado da UI ----------

let config: Config = { monitorados: [], ignorados: [], intervalo_ms: 2000, notificacoes: true, nome_rede: "" };
let estado: Estado | null = null;
let tela: Tela = "principal";
let editando: Monitorado | null = null; // null = novo
let mostrarOcultas = false;
let confirmandoRemocao: string | null = null;
let arrastando: string | null = null;
let caminhoConfig = "";

const $app = document.getElementById("app")!;
const $resumo = document.getElementById("resumo")!;
const $titulo = document.getElementById("titulo")!;
const $voltar = document.getElementById("btn-voltar") as HTMLButtonElement;
const $adicionar = document.getElementById("btn-adicionar") as HTMLButtonElement;
const $btnConfig = document.getElementById("btn-config") as HTMLButtonElement;
const $toast = document.getElementById("toast")!;

// ---------- utilidades ----------

const esc = (s: unknown): string =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

function novoId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

function caminhoUrl(m: Monitorado): string {
  const c = m.caminho.trim();
  return !c ? "" : c.startsWith("/") ? c : `/${c}`;
}

const urlLocal = (m: Monitorado) => `${m.protocolo}://localhost:${m.porta}${caminhoUrl(m)}`;

function urlsRede(m: Monitorado): { ip: string | null; mdns: string | null } {
  const r = estado?.rede;
  const sufixo = `:${m.porta}${caminhoUrl(m)}`;
  return {
    ip: r?.ip_lan ? `${m.protocolo}://${r.ip_lan}${sufixo}` : null,
    mdns: nomeLocal() ? `${m.protocolo}://${nomeLocal()}${sufixo}` : null,
  };
}

const NOME_VALIDO = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

/** Nome .local para copiar: o personalizado quando publicado, senão o do computador. */
function nomeLocal(): string | null {
  const n = estado?.nome_rede;
  if (n?.situacao === "publicado" && n.nome) return n.nome;
  return estado?.rede.hostname ? `${estado.rede.hostname}.local` : null;
}

function statusNomeHtml(): string {
  const n = estado?.nome_rede;
  const host = estado?.rede.hostname ? `${esc(estado.rede.hostname)}.local` : "?";
  let personalizado: string;
  if (!config.nome_rede) personalizado = `<span class="dica">desligado</span>`;
  else if (!n || n.situacao === "publicando" || n.nome !== `${config.nome_rede}.local`)
    personalizado = `<strong>${esc(config.nome_rede)}.local</strong> <span class="selo local">publicando…</span>`;
  else if (n.situacao === "publicado")
    personalizado = `<strong>${esc(n.nome)}</strong> → ${esc(n.ip ?? "")} <span class="selo rede">publicado</span>`;
  else
    personalizado = `<strong>${esc(n.nome)}</strong> <span class="selo erro">erro</span><div class="aviso">${esc(n.mensagem ?? "Falha ao publicar")}</div>`;
  const r = estado?.rede;
  return `<p class="dica">IP da LAN: <strong>${esc(r?.ip_lan ?? "não encontrado")}</strong>${r?.interface ? ` (${esc(r.interface)})` : ""}</p>
    <p class="dica">Nome personalizado: ${personalizado}</p>
    <p class="dica">Nome do computador: <strong>${host}</strong> <span class="dica">(sempre ativo, via avahi)</span></p>`;
}

const statusDe = (id: string) => estado?.status.find((s) => s.id === id);

let toastTimer = 0;
function toast(msg: string) {
  $toast.textContent = msg;
  $toast.classList.add("visivel");
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => $toast.classList.remove("visivel"), 2200);
}

async function copiar(texto: string) {
  try {
    await writeText(texto);
    toast(`Copiado: ${texto}`);
  } catch (e) {
    toast(`Não consegui copiar: ${e}`);
  }
}

async function abrir(url: string) {
  try {
    await openUrl(url);
  } catch (e) {
    toast(`Não consegui abrir: ${e}`);
  }
}

async function salvar(nova: Config): Promise<boolean> {
  let ok = true;
  try {
    config = await invoke<Config>("salvar_config", { config: nova });
  } catch (e) {
    toast(String(e));
    ok = false;
  }
  render();
  return ok;
}

function nomeSugerido(d: Descoberto): string {
  const pasta = d.processo?.pasta;
  if (pasta && pasta !== "~" && pasta !== "/" && !pasta.startsWith("/usr")) {
    const partes = pasta.split("/").filter(Boolean);
    return partes[partes.length - 1] ?? pasta;
  }
  return d.processo?.programa ?? `Porta ${d.porta}`;
}

function processoTexto(p: Processo | null, outro: boolean): string {
  if (!p) return outro ? "processo de outro usuário (sem acesso)" : "processo desconhecido";
  return p.pasta ? `${p.programa} · ${p.pasta}` : p.programa;
}

// ---------- telas ----------

function render() {
  $voltar.hidden = tela === "principal";
  $adicionar.hidden = tela !== "principal";
  $btnConfig.hidden = tela !== "principal";
  $titulo.textContent = tela === "principal" ? "Porteiro" : tela === "config" ? "Configurações" : editando ? "Editar projeto" : "Adicionar projeto";
  renderResumo();
  if (tela === "principal") renderPrincipal();
  else if (tela === "form") renderForm();
  else renderConfig();
}

function renderResumo() {
  const total = config.monitorados.length;
  const online = config.monitorados.filter((m) => statusDe(m.id)?.online).length;
  $resumo.textContent = tela === "principal" && total ? `${online} de ${total} online` : "";
}

function cardMonitorado(m: Monitorado, i: number): string {
  const st = statusDe(m.id);
  const carregando = !estado;
  const online = !!st?.online;
  const classe = carregando ? "carregando" : online ? "online" : "offline";
  const rotulo = carregando ? "…" : online ? "online" : "offline";
  const escopo = online && st?.escopo
    ? `<span class="selo ${st.escopo}">${st.escopo === "rede" ? "na rede" : "só local"}</span>`
    : "";
  const proc = online ? `<div class="proc" title="${esc(st?.processo?.cmdline ?? "")}">${esc(processoTexto(st?.processo ?? null, false))}</div>` : "";
  const aviso = online && st?.escopo === "local"
    ? `<div class="aviso">Colegas da rede não conseguem acessar: o servidor escuta só em ${esc(st.enderecos.join(", ") || "127.0.0.1")}.</div>`
    : "";
  const rede = urlsRede(m);
  const removendo = confirmandoRemocao === m.id;
  return `
  <li class="card ${classe}" draggable="true" data-id="${esc(m.id)}">
    <div class="linha1">
      <span class="alca" title="Arraste para reordenar">⋮⋮</span>
      <span class="bolinha" aria-hidden="true"></span>
      <div class="nome-bloco">
        <div class="nome">${esc(m.nome)}</div>
        <div class="endereco">${esc(urlLocal(m))}</div>
      </div>
      <span class="status-texto">${rotulo}</span>
    </div>
    <div class="detalhes">${escopo}${proc}</div>
    ${aviso}
    ${m.observacao ? `<div class="obs">${esc(m.observacao)}</div>` : ""}
    <div class="acoes">
      <button data-acao="abrir" ${online ? "" : 'class="fraco"'} title="${esc(urlLocal(m))}">Abrir</button>
      <button data-acao="copiar-ip" ${rede.ip ? "" : "disabled"} title="${esc(rede.ip ?? "Sem IP de rede")}">Copiar IP</button>
      <button data-acao="copiar-mdns" title="${esc(rede.mdns ?? "")}">Copiar .local</button>
      <span class="espaco"></span>
      <button class="icone ${m.notificar ? "" : "desligado"}" data-acao="notificar" title="${m.notificar ? "Notificações ligadas para este projeto" : "Notificações desligadas para este projeto"}">${m.notificar ? "🔔" : "🔕"}</button>
      <button class="icone" data-acao="subir" title="Mover para cima" ${i === 0 ? "disabled" : ""}>↑</button>
      <button class="icone" data-acao="descer" title="Mover para baixo" ${i === config.monitorados.length - 1 ? "disabled" : ""}>↓</button>
      <button class="icone" data-acao="editar" title="Editar">✎</button>
      <button class="icone ${removendo ? "perigo confirmar" : ""}" data-acao="remover" title="Remover">${removendo ? "Remover?" : "✕"}</button>
    </div>
  </li>`;
}

function renderPrincipal() {
  if (!config.monitorados.length) {
    $app.innerHTML = `
      <div class="vazio">
        <p><strong>Nenhum projeto monitorado.</strong></p>
        <p>Clique em <em>+ Adicionar</em> para escolher quais servidores locais você quer acompanhar.</p>
      </div>`;
    return;
  }
  $app.innerHTML = `<ul class="lista" id="lista">${config.monitorados.map(cardMonitorado).join("")}</ul>`;
}

function sugestoesHtml(): string {
  if (!estado) return `<p class="dica">Lendo as portas…</p>`;
  const candidatas = estado.descobertos.filter((d) => !d.monitorado);
  const visiveis = candidatas.filter((d) => mostrarOcultas || !d.ignorado);
  const ocultas = candidatas.filter((d) => d.ignorado).length;
  const itens = visiveis
    .map(
      (d) => `
      <li class="sugestao ${d.ignorado ? "oculta" : ""}" data-porta="${d.porta}" tabindex="0" title="${esc(d.processo?.cmdline ?? "")}">
        <span class="porta">:${d.porta}</span>
        <div class="sug-info">
          <div class="sug-nome">${esc(d.processo?.programa ?? "—")} <span class="selo ${d.escopo}">${d.escopo === "rede" ? "na rede" : "só local"}</span></div>
          <div class="sug-sub">${esc(d.processo?.pasta ?? (d.outro_usuario ? "de outro usuário" : d.enderecos.join(", ")))}</div>
        </div>
        <span class="usar">usar</span>
      </li>`,
    )
    .join("");
  const rodape = ocultas
    ? `<button class="link" id="alternar-ocultas">${mostrarOcultas ? "Esconder" : "Mostrar"} ${ocultas} oculta${ocultas > 1 ? "s" : ""} pelos filtros</button>`
    : "";
  return `${itens ? `<ul class="sugestoes">${itens}</ul>` : `<p class="dica">Nada novo escutando agora.</p>`}${rodape}`;
}

function renderForm() {
  const m: Monitorado = editando ?? { id: "", nome: "", porta: 0, caminho: "", protocolo: "http", observacao: "", notificar: true };
  $app.innerHTML = `
    <form id="form" class="form" autocomplete="off">
      <label>Nome<input name="nome" required maxlength="80" value="${esc(m.nome)}" placeholder="Ex.: Front da loja"></label>
      <div class="lado">
        <label class="porta-campo">Porta<input name="porta" type="number" min="1" max="65535" required value="${m.porta || ""}" placeholder="3000"></label>
        <label>Caminho (opcional)<input name="caminho" value="${esc(m.caminho)}" placeholder="/admin"></label>
        <label class="proto">Protocolo<select name="protocolo">
          <option value="http" ${m.protocolo === "http" ? "selected" : ""}>http</option>
          <option value="https" ${m.protocolo === "https" ? "selected" : ""}>https</option>
        </select></label>
      </div>
      <label>Observação<input name="observacao" maxlength="200" value="${esc(m.observacao)}" placeholder="Ex.: npm run dev na pasta loja/"></label>
      <label class="check"><input type="checkbox" name="notificar" ${m.notificar ? "checked" : ""}> Notificar quando cair ou voltar</label>
      <div class="botoes">
        <button type="button" id="cancelar">Cancelar</button>
        <button type="submit" class="primario">${editando ? "Salvar" : "Adicionar"}</button>
      </div>
    </form>
    ${editando ? "" : `
    <section class="secao">
      <h2>Escutando agora <span class="dica">— clique para preencher</span></h2>
      <div id="sugestoes">${sugestoesHtml()}</div>
    </section>`}`;
  (document.querySelector('input[name="nome"]') as HTMLInputElement)?.focus();
}

function renderConfig() {
  const regras = config.ignorados
    .map(
      (g, i) => `<li class="regra"><span>${g.programa ? `<strong>${esc(g.programa)}</strong>` : ""}${g.programa && g.porta ? " na " : ""}${g.porta ? `porta <strong>${g.porta}</strong>` : ""}</span>
        <button class="icone" data-remover-regra="${i}" title="Remover filtro">✕</button></li>`,
    )
    .join("");
  $app.innerHTML = `
    <section class="secao">
      <h2>Geral</h2>
      <label class="check"><input type="checkbox" id="cfg-notif" ${config.notificacoes ? "checked" : ""}> Notificações (projeto caiu / voltou)</label>
      <label class="inline">Atualizar a cada
        <select id="cfg-intervalo">
          ${[1000, 2000, 3000, 5000, 10000, 30000].map((v) => `<option value="${v}" ${config.intervalo_ms === v ? "selected" : ""}>${v / 1000} s</option>`).join("")}
        </select></label>
      <div class="botoes esquerda"><button id="testar-notif">Testar notificação</button></div>
    </section>
    <section class="secao">
      <h2>Rede</h2>
      <form id="form-nome" class="lado" autocomplete="off">
        <label>Nome na rede<input name="nome_rede" value="${esc(config.nome_rede)}" placeholder="ex.: painel-app" maxlength="63" pattern="[a-z0-9]([a-z0-9\-]*[a-z0-9])?" spellcheck="false"></label>
        <span class="sufixo">.local</span>
        <button type="submit">Salvar</button>
      </form>
      <p class="dica">Publica um nome extra <code>&lt;nome&gt;.local</code> apontando para o IP da LAN, para colegas abrirem os projetos por nome. O hostname e a configuração de rede do computador <strong>não mudam</strong>; o nome extra só existe enquanto o Porteiro está aberto (deixe-o iniciar com o sistema, com <code>--oculto</code>). Vazio = desligado.</p>
      <div id="status-nome">${statusNomeHtml()}</div>
    </section>
    <section class="secao">
      <h2>Filtros das sugestões</h2>
      <p class="dica">Programas e portas que não aparecem como sugestão ao adicionar (ruído como Steam, Discord, VS Code). Não afetam os projetos monitorados.</p>
      <ul class="regras">${regras || `<li class="dica">Nenhum filtro.</li>`}</ul>
      <form id="nova-regra" class="lado" autocomplete="off">
        <label>Programa<input name="programa" placeholder="ex.: steam"></label>
        <label class="porta-campo">Porta<input name="porta" type="number" min="1" max="65535" placeholder="—"></label>
        <button type="submit">Adicionar</button>
      </form>
      <div class="botoes esquerda"><button class="link" id="restaurar-padrao">Restaurar filtros padrão</button></div>
    </section>
    <section class="secao">
      <h2>Sobre</h2>
      <p class="dica">Configuração em <code>${esc(caminhoConfig)}</code>. Fechar a janela deixa o app na bandeja; para sair use “Sair” no menu da bandeja.</p>
    </section>`;
}

/** Atualização vinda do backend sem destruir o que o usuário está digitando. */
function aoReceberEstado(novo: Estado) {
  estado = novo;
  renderResumo();
  if (tela === "principal") {
    if (!arrastando) renderPrincipal();
  } else if (tela === "config") {
    const alvo = document.getElementById("status-nome");
    if (alvo) alvo.innerHTML = statusNomeHtml();
  } else if (tela === "form" && !editando) {
    const alvo = document.getElementById("sugestoes");
    if (alvo) alvo.innerHTML = sugestoesHtml();
  }
}

// ---------- ações ----------

function irPara(t: Tela, m: Monitorado | null = null) {
  tela = t;
  editando = m;
  confirmandoRemocao = null;
  render();
  window.scrollTo(0, 0);
}

function mover(id: string, destino: number) {
  const lista = [...config.monitorados];
  const de = lista.findIndex((m) => m.id === id);
  if (de < 0 || destino < 0 || destino >= lista.length || de === destino) return;
  const [item] = lista.splice(de, 1);
  lista.splice(destino, 0, item);
  salvar({ ...config, monitorados: lista });
}

function acaoCard(id: string, acao: string) {
  const m = config.monitorados.find((x) => x.id === id);
  if (!m) return;
  const i = config.monitorados.indexOf(m);
  if (acao !== "remover") confirmandoRemocao = null;
  switch (acao) {
    case "abrir":
      abrir(urlLocal(m));
      break;
    case "copiar-ip": {
      const u = urlsRede(m).ip;
      if (u) copiar(u);
      break;
    }
    case "copiar-mdns": {
      const u = urlsRede(m).mdns;
      if (u) copiar(u);
      break;
    }
    case "notificar":
      salvar({ ...config, monitorados: config.monitorados.map((x) => (x.id === id ? { ...x, notificar: !x.notificar } : x)) });
      break;
    case "subir":
      mover(id, i - 1);
      break;
    case "descer":
      mover(id, i + 1);
      break;
    case "editar":
      irPara("form", m);
      break;
    case "remover":
      if (confirmandoRemocao === id) {
        confirmandoRemocao = null;
        salvar({ ...config, monitorados: config.monitorados.filter((x) => x.id !== id) });
        toast(`“${m.nome}” removido`);
      } else {
        confirmandoRemocao = id;
        renderPrincipal();
        setTimeout(() => {
          if (confirmandoRemocao === id) {
            confirmandoRemocao = null;
            if (tela === "principal") renderPrincipal();
          }
        }, 3000);
      }
      break;
  }
}

function enviarForm(form: HTMLFormElement) {
  const d = new FormData(form);
  const porta = Number(d.get("porta"));
  if (!Number.isInteger(porta) || porta < 1 || porta > 65535) {
    toast("Porta inválida (1–65535)");
    return;
  }
  const nome = String(d.get("nome") ?? "").trim() || `Porta ${porta}`;
  const m: Monitorado = {
    id: editando?.id ?? novoId(),
    nome,
    porta,
    caminho: String(d.get("caminho") ?? "").trim(),
    protocolo: d.get("protocolo") === "https" ? "https" : "http",
    observacao: String(d.get("observacao") ?? "").trim(),
    notificar: d.get("notificar") === "on",
  };
  const monitorados = editando
    ? config.monitorados.map((x) => (x.id === m.id ? m : x))
    : [...config.monitorados, m];
  tela = "principal";
  editando = null;
  salvar({ ...config, monitorados });
  toast(`“${m.nome}” salvo`);
}

function preencherComSugestao(porta: number) {
  const d = estado?.descobertos.find((x) => x.porta === porta);
  if (!d) return;
  const form = document.getElementById("form") as HTMLFormElement | null;
  if (!form) return;
  (form.elements.namedItem("nome") as HTMLInputElement).value = nomeSugerido(d);
  (form.elements.namedItem("porta") as HTMLInputElement).value = String(d.porta);
  const obs = form.elements.namedItem("observacao") as HTMLInputElement;
  if (!obs.value && d.processo?.cmdline) obs.value = d.processo.cmdline.slice(0, 200);
  (form.elements.namedItem("nome") as HTMLInputElement).focus();
}

// ---------- eventos (delegação) ----------

$voltar.addEventListener("click", () => irPara("principal"));
$adicionar.addEventListener("click", () => irPara("form"));
$btnConfig.addEventListener("click", () => irPara("config"));
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && tela !== "principal") irPara("principal");
});

$app.addEventListener("click", (e) => {
  const alvo = e.target as HTMLElement;
  const botao = alvo.closest<HTMLElement>("[data-acao]");
  const card = alvo.closest<HTMLElement>(".card");
  if (botao && card) {
    acaoCard(card.dataset.id!, botao.dataset.acao!);
    return;
  }
  const sug = alvo.closest<HTMLElement>(".sugestao");
  if (sug) {
    preencherComSugestao(Number(sug.dataset.porta));
    return;
  }
  if (alvo.id === "alternar-ocultas") {
    mostrarOcultas = !mostrarOcultas;
    const s = document.getElementById("sugestoes");
    if (s) s.innerHTML = sugestoesHtml();
    return;
  }
  if (alvo.id === "cancelar") {
    irPara("principal");
    return;
  }
  if (alvo.id === "testar-notif") {
    invoke("testar_notificacao").catch((err) => toast(String(err)));
    return;
  }
  if (alvo.id === "restaurar-padrao") {
    invoke<Ignorado[]>("ignorados_padrao").then((ignorados) => salvar({ ...config, ignorados }));
    return;
  }
  const rem = alvo.closest<HTMLElement>("[data-remover-regra]");
  if (rem) {
    const i = Number(rem.dataset.removerRegra);
    salvar({ ...config, ignorados: config.ignorados.filter((_, j) => j !== i) });
  }
});

$app.addEventListener("keydown", (e) => {
  const sug = (e.target as HTMLElement).closest<HTMLElement>(".sugestao");
  if (sug && (e.key === "Enter" || e.key === " ")) {
    e.preventDefault();
    preencherComSugestao(Number(sug.dataset.porta));
  }
});

$app.addEventListener("submit", (e) => {
  e.preventDefault();
  const form = e.target as HTMLFormElement;
  if (form.id === "form") enviarForm(form);
  else if (form.id === "form-nome") {
    const nome = String(new FormData(form).get("nome_rede") ?? "").trim();
    if (nome && !NOME_VALIDO.test(nome)) {
      toast("Use só letras minúsculas, números e hífen");
      return;
    }
    salvar({ ...config, nome_rede: nome }).then((ok) => ok && toast(nome ? `Publicando ${nome}.local…` : "Nome na rede desligado"));
  }
  else if (form.id === "nova-regra") {
    const d = new FormData(form);
    const programa = String(d.get("programa") ?? "").trim() || null;
    const portaTxt = String(d.get("porta") ?? "").trim();
    const porta = portaTxt ? Number(portaTxt) : null;
    if (!programa && !porta) {
      toast("Informe um programa e/ou uma porta");
      return;
    }
    salvar({ ...config, ignorados: [...config.ignorados, { programa, porta }] });
  }
});

$app.addEventListener("change", (e) => {
  const alvo = e.target as HTMLInputElement;
  if (alvo.id === "cfg-notif") salvar({ ...config, notificacoes: alvo.checked });
  if (alvo.id === "cfg-intervalo") salvar({ ...config, intervalo_ms: Number(alvo.value) });
});

// Arrastar para reordenar.
$app.addEventListener("dragstart", (e) => {
  const card = (e.target as HTMLElement).closest<HTMLElement>(".card");
  if (!card) return;
  arrastando = card.dataset.id!;
  card.classList.add("arrastando");
  e.dataTransfer?.setData("text/plain", arrastando);
  if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
});
$app.addEventListener("dragover", (e) => {
  const card = (e.target as HTMLElement).closest<HTMLElement>(".card");
  if (!arrastando || !card) return;
  e.preventDefault();
  document.querySelectorAll(".card.alvo").forEach((c) => c.classList.remove("alvo"));
  if (card.dataset.id !== arrastando) card.classList.add("alvo");
});
$app.addEventListener("drop", (e) => {
  const card = (e.target as HTMLElement).closest<HTMLElement>(".card");
  if (!arrastando || !card) return;
  e.preventDefault();
  const destino = config.monitorados.findIndex((m) => m.id === card.dataset.id);
  const id = arrastando;
  arrastando = null;
  mover(id, destino);
});
$app.addEventListener("dragend", () => {
  arrastando = null;
  document.querySelectorAll(".card.arrastando, .card.alvo").forEach((c) => c.classList.remove("arrastando", "alvo"));
});

// ---------- início ----------

async function iniciar() {
  [config, estado, caminhoConfig] = await Promise.all([
    invoke<Config>("obter_config"),
    invoke<Estado | null>("obter_estado"),
    invoke<string>("caminho_config"),
  ]);
  const inicial = await invoke<Tela | null>("tela_inicial");
  if (inicial) irPara(inicial);
  else render();
  await listen<Estado>("estado", (ev) => aoReceberEstado(ev.payload));
  await listen<Tela>("ir-para", (ev) => irPara(ev.payload));
  invoke("atualizar_agora");
}

iniciar().catch((e) => {
  $app.innerHTML = `<p class="erro">Erro ao iniciar: ${esc(e)}</p>`;
});
