# Porteiro

Aplicativo desktop (Linux) que acompanha **só os servidores locais que você escolheu** — o front na porta 3000, o painel na 4747 etc. — e mostra se estão online, se os colegas da rede conseguem acessar e de qual pasta/processo vieram. Fica na bandeja do sistema e avisa quando um projeto cai ou volta.

Feito com Tauri v2 (Rust) + Vite/TypeScript sem framework.

## Como usar

- **Tela principal**: a lista dos projetos monitorados. Cada um mostra:
  - ● online / ○ offline (online = existe algum processo escutando TCP na porta);
  - **na rede** ou **só local** — quando está só local aparece o aviso “Colegas da rede não conseguem acessar: o servidor escuta só em 127.0.0.1” (rode o servidor com `--host 0.0.0.0`, `HOST=0.0.0.0` ou equivalente);
  - o processo e a pasta (`node · ~/Desenvolvimento/meu-projeto`);
  - ações: **Abrir** (`http://localhost:<porta><caminho>` no navegador), **Copiar IP** (`http://<IP da LAN>:<porta>`), **Copiar .local** (`http://<hostname>.local:<porta>`), 🔔 notificações deste projeto, ↑/↓ ou arrastar pela alça `⋮⋮` para reordenar, ✎ editar e ✕ remover (clique duas vezes para confirmar).
- **+ Adicionar**: formulário com nome, porta, caminho opcional (ex.: `/admin`), protocolo (http/https), observação e “notificar”. Embaixo aparece a lista **Escutando agora** com o que está ouvindo TCP na máquina (porta, programa, pasta, local/rede); clicar numa sugestão preenche o formulário. O ruído (Steam, Discord, VS Code, `language_server`, adb, systemd-resolved, cups…) e o que já é monitorado ficam de fora; dá para mostrar os ocultos.
- **⚙ Configurações**: liga/desliga notificações, intervalo de atualização (padrão 2 s), testar notificação, **nome na rede** (veja abaixo), IP da LAN detectado, e os **filtros das sugestões** (por programa e/ou porta; editáveis, com “restaurar padrão”).
- **Bandeja**: menu com “Porteiro — N de M online”, cada projeto (● online / ○ offline; clicar abre no navegador), “Mostrar janela” e “Sair”. Fechar a janela só esconde o app na bandeja; para encerrar use **Sair**.
- **Notificações**: “Projeto X caiu (porta 3000)” / “Projeto X voltou”. Não notifica na primeira leitura ao abrir o app.

### Nome na rede personalizado (`<nome>.local`)

Em Configurações → Rede, o campo **Nome na rede** (só letras minúsculas, números e hífen; vazio = desligado) publica um nome extra, por exemplo `painel-app.local`, apontando para o IP da LAN. Assim os colegas abrem `http://painel-app.local:4747/` em vez de decorar o IP. O **Copiar .local** dos projetos passa a usar esse nome enquanto ele estiver publicado.

- **Nada do sistema muda**: o hostname (`NF-ENG-Sergio`) e a configuração de rede continuam iguais, e `NF-ENG-Sergio.local` segue funcionando. Não precisa de sudo.
- Funciona com `avahi-publish -a -R <nome>.local <ip>` (pacote `avahi`, daemon ativo). O processo filho morre junto com o app (Sair, fechar a sessão, SIGTERM ou SIGKILL), então **o nome extra só existe com o Porteiro aberto**. Recomendo iniciar com o sistema com `--oculto` (veja a seção do niri).
- Se o IP da LAN mudar, o nome é republicado no IP novo. Se o nome mudar, o antigo sai e o novo entra.
- Se outro aparelho da rede já usa o nome, aparece “painel-app.local já está em uso na rede” em Configurações e o app tenta de novo a cada 30 s. Erros do avahi também aparecem lá.
- O IP publicado segue a mesma regra do “Copiar IP” (prefere IP privado e cabo `en*` antes de Wi-Fi, ignorando interfaces virtuais) e é mostrado em Configurações.

### Linha de comando

| Opção | Efeito |
|---|---|
| `--oculto` | inicia só na bandeja, sem mostrar a janela |
| `--adicionar` | abre direto na tela “Adicionar” (se já estiver aberto, traz a janela para frente nessa tela) |
| `--config` | abre direto nas Configurações |
| `--dump` | imprime em JSON o que está escutando e o status dos monitorados, e sai (sem janela) |

Só roda uma instância: abrir de novo apenas mostra a janela existente.

## Configuração

Fica em `~/.config/porteiro/config.json` (ou `$XDG_CONFIG_HOME/porteiro/config.json`), é gravada a cada mudança e sobrevive a trocar o AppImage. Se você usava a versão chamada Localhosts, a config de `~/.config/localhosts/` é copiada automaticamente na primeira execução. Na primeira execução (se o arquivo não existir) é criada com os filtros padrão e dois exemplos: “Painel Maestro App Aluno” (4747) e “Painel Maestro App Pro” (4748).

```json
{
  "monitorados": [
    { "id": "painel-app-aluno", "nome": "Painel Maestro App Aluno", "porta": 4747,
      "caminho": "", "protocolo": "http", "observacao": "", "notificar": true }
  ],
  "ignorados": [ { "programa": "steam", "porta": null }, { "programa": null, "porta": 631 } ],
  "intervalo_ms": 2000,
  "notificacoes": true,
  "nome_rede": "painel-app"
}
```

Um filtro com programa **e** porta só esconde quando os dois batem. O programa é comparado (sem diferenciar maiúsculas) com o `comm` do processo e com o nome do executável.

## Desenvolvimento

Pré-requisitos (Arch/CachyOS): `rust`, `nodejs`, `npm`, `webkit2gtk-4.1`, `gtk3`, `libayatana-appindicator`, `librsvg` (só para regenerar ícones).

```sh
npm install
npm run tauri dev        # app em modo desenvolvimento
npm run build            # só o frontend (tsc + vite)
npm run dump             # lista o que está escutando, em JSON
cd src-tauri && cargo test && cargo clippy --all-targets
npm run icons            # regenera os PNGs a partir de src-tauri/icons/icon.svg
```

## Gerar o AppImage

```sh
npm run appimage
# equivale a: NO_STRIP=true npm run tauri build -- --bundles appimage
```

Saída: `src-tauri/target/release/bundle/appimage/Porteiro_<versão>_amd64.AppImage`.

`NO_STRIP=true` é necessário no Arch/CachyOS: o `strip` embutido no linuxdeploy é antigo e falha nas bibliotecas do sistema (formato `.relr.dyn`). O binário do app já sai sem símbolos (`strip = true` no `Cargo.toml`).

O AppImage tem ~100 MB porque leva junto o WebKitGTK e as bibliotecas GTK (é como o bundler do Tauri faz no Linux). O binário em si tem poucos MB. Para rodar o AppImage é preciso `fuse2` (ou extrair com `--appimage-extract`).

## Iniciar com o sistema (niri)

Copie o AppImage para um lugar fixo (ex.: `~/Aplicativos/Porteiro.AppImage`, `chmod +x`) e adicione em `~/.config/niri/config.kdl`:

```kdl
spawn-at-startup "~/Aplicativos/Porteiro.AppImage" "--oculto"
```

(Se o seu niri não expandir `~`, use o caminho absoluto.) Opcionalmente um atalho para adicionar projeto:

```kdl
binds {
    Mod+Shift+L { spawn "~/Aplicativos/Porteiro.AppImage" "--adicionar"; }
}
```

## Como funciona

A cada intervalo o backend (Rust, fora da thread da UI) lê `/proc/net/tcp` e `/proc/net/tcp6` (estado `0A` = LISTEN), mapeia o inode do socket para o PID varrendo `/proc/<pid>/fd`, e lê `/proc/<pid>/comm`, `cmdline`, `exe` e `cwd`. Bind em `127.x`/`::1` = **só local**; `0.0.0.0`, `::` ou IP da LAN = **na rede**. O IP da LAN ignora interfaces virtuais (`lo`, `docker*`, `virbr*`, `waydroid*`, `veth*`, `br-*`, VPNs…). O mesmo laço atualiza a bandeja e dispara as notificações, então tudo continua funcionando com a janela escondida.

## Limitações

- Processos de **outros usuários ou do root** (cupsd, systemd-resolved, docker-proxy…) aparecem **sem programa nem pasta**: o Linux não deixa ler o `/proc/<pid>/fd` deles sem privilégio. O status online/offline funciona normalmente.
- Só TCP. “Online” significa que algo escuta na porta, não que o HTTP responde.
- Servidores dentro de contêineres/VM aparecem como o processo de encaminhamento (ex.: `docker-proxy`, de root).
- O nome extra `<nome>.local` só existe enquanto o app está aberto.
- O endereço `http://<hostname>.local:<porta>` (e o nome extra) depende de mDNS (avahi) na sua máquina **e** na de quem acessa, e de a rede não bloquear multicast. O IP da LAN é o caminho mais garantido.
- Tooltip da bandeja: o backend do Tauri no Linux (libayatana-appindicator) não suporta tooltip; por isso o resumo “N de M online” aparece como primeira linha do menu.
- “Iniciar com o sistema” ainda não é uma opção do app (veja a seção do niri).
