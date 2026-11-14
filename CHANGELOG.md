# Changelog

Todas as mudanças relevantes do Painel LED ficam registradas aqui.
O formato segue o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/), e as versões seguem o [SemVer](https://semver.org/lang/pt-BR/).

## [0.3.0] - 2026-11-14

### Adicionado
- Escolha do monitor na aba Painel. Os monitores aparecem desenhados na proporção e na posição reais; um clique manda o painel para lá na hora. "Automático" mantém o comportamento de antes (fica onde for arrastado). Com um monitor fixo, arrastar o painel para outro monitor troca a escolha. Configuração: `painel.monitor`.
- Opção "Reservar espaço" para as posições Topo e Base. O painel vira uma barra do sistema, e janelas maximizadas param antes dele em vez de ficarem por baixo. Vale só para o monitor escolhido. No Windows usa a AppBar (a mesma API da barra de tarefas); no Linux, uma janela Dock com `_NET_WM_STRUT_PARTIAL` (X11 e XWayland). Configuração: `painel.reservar_espaco`.
- A conta logada no Claude Code (e-mail, plano e organização) aparece na aba Conectar ao Claude.
- O aviso de término passou a dizer o que o Claude fez, com o começo do resumo final: `CLAUDE [api]: terminou: Corrigi os 3 testes de login.`. Para voltar ao aviso simples, use `claude.detalhar = false`.
- Na tela de configurações, passar o mouse numa sessão mostra o último pedido e o resumo da resposta. A ferramenta MCP `sessoes_claude` e a rota `GET /claude` também trazem esses dois campos (`pedido` e `resposta`).
- `cargo audit` no CI, e `.cargo/audit.toml` com os avisos ignorados de propósito e o motivo de cada um.
- Este CHANGELOG.

### Alterado
- O item `sessoes` começa com `CLAUDE`, para deixar claro de que é a lista. Nomes de projeto com mais de 16 letras terminam em `..`.
- No Linux, o ícone da bandeja usa o protocolo StatusNotifierItem via D-Bus (backend `ksni`), sem GTK. O binário passa a depender só da `libc`, e o pacote `.deb` não exige mais GTK nem AppIndicator. O CI não instala mais bibliotecas de desenvolvimento.
- O `winit` é compilado sem as bordas desenhadas do Wayland (`wayland-csd-adwaita`), que o painel não usa.

### Corrigido
- No Linux, a reserva de espaço podia não ser gravada, porque a conexão X11 fechava antes de o servidor processar o pedido. Erros nessa parte agora aparecem no log.

### Segurança
- O `glib` 0.18 saiu das dependências. Ele era afetado pelo RUSTSEC-2024-0429 (código "unsound" em `glib::VariantStrIter`), corrigido só no `glib` 0.20, que não existe para GTK 3. Entrava pelo ícone da bandeja no Linux.
- Saíram também o `proc-macro-error` e o `ttf-parser`, ambos sem manutenção.

## [0.2.0] - 2026-11-14

### Adicionado
- Suporte a Linux: pacote `.deb` (Ubuntu, Debian, Mint, Pop!_OS) e `.tar.gz` com `install.sh`, que instala em `~/.local` sem root.
- Atalho no menu de aplicativos (`.desktop`) e ícone PNG.
- No Linux, a tela de configurações abre em modo app do Chrome, Chromium, Edge ou Brave, se houver algum.
- No Wayland, o painel abre pelo XWayland, para manter "sempre no topo" e a posição escolhida. `PAINEL_LED_WAYLAND=1` força o Wayland nativo.
- `PAINEL_LED_JANELA_NORMAL=1` usa uma janela comum, para gerenciadores de janelas que escondem janelas utilitárias. No WSL isso já é automático.
- O workflow passou a gerar uma Release única com os arquivos de Windows e de Linux.

## [0.1.0] - 2026-11-14

### Adicionado
- Painel de LED virtual sempre no topo, desenhado em software (winit + softbuffer), com uns 12 MB de memória.
- Itens: relógio, data, CPU/RAM, latência e histórico de 24 h da internet, Google Agenda (iCal, com recorrência e avisos), textos fixos e textos via API.
- Linhas divididas em blocos, com efeitos fixo, rolar e piscar, e largura e cor por bloco.
- Sessões do Claude Code (hooks) e limites de uso do plano (statusline), com avisos de permissão, término e limite.
- Servidor MCP (`painel-led-mcp`) com ferramentas para mensagens, alertas, lembretes, textos e status.
- API HTTP local em `127.0.0.1:7878`, aceita só chamadas da própria máquina, com token para alterar a configuração.
- Tela de configurações com abas, explicações em cada opção e botões para conectar o MCP, os hooks e a statusline.
- Ícone na bandeja, opção de iniciar com o Windows, e instalador do Windows (Inno Setup) com versão portátil, gerados pelo GitHub Actions.

[0.3.0]: https://github.com/berlotto/painel-led-auxiliar/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/berlotto/painel-led-auxiliar/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/berlotto/painel-led-auxiliar/releases/tag/v0.1.0
