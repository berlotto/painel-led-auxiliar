//! Configuração em TOML (`%APPDATA%\painel-led\config.toml`), recarregada ao salvar.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub const DEFAULT_TOML: &str = r##"# Painel LED: configuração
# Salve o arquivo e o painel recarrega sozinho.
# Clique direito no painel abre este arquivo.

[painel]
largura = "60%"        # "100%" = largura total do monitor, "60%", ou pixels: "900"
linhas = 1             # linhas de texto (cada linha tem 7 LEDs de altura)
tamanho_led = 8        # distância entre LEDs, em pixels
posicao = "livre"      # "topo", "base" ou "livre" (arraste; a posição é lembrada)
velocidade = 28        # colunas por segundo ao rolar
brilho = 100           # 10 a 100 (%)
linha_mensagens = 1    # em qual linha aparecem as mensagens da API / Claude
monitor = 0            # 0 = onde você arrastar; 1, 2... = monitor fixo (da esquerda para a direita)
reservar_espaco = false # com posicao "topo" ou "base": janelas maximizadas não cobrem o painel

# Cada [[linha]] é dividida em blocos lado a lado.
# itens: relogio, data, sistema, cpu, ram, rede, historico, agenda, sessoes, limites,
#        "texto:Qualquer texto", "var:nome" (texto definido pela API POST /texto)
# efeito: "fixo" (troca de item a cada `segundos`), "rolar" (letreiro) ou "piscar"
# largura: em LEDs; sem largura, o bloco ocupa o espaço que sobrar
# cor: verde, ambar, vermelho, ciano, branco, laranja, rosa, azul, amarelo ou "#rrggbb"

[[linha]]
blocos = [
  { itens = ["relogio"], largura = 31 },
  { itens = ["data", "sistema", "rede", "historico", "agenda", "sessoes", "limites"], efeito = "fixo", segundos = 5 },
]

# Exemplo de segunda linha (use linhas = 2 lá em cima):
# [[linha]]
# blocos = [
#   { itens = ["texto:Foco total hoje!"], efeito = "rolar", cor = "verde" },
# ]

[agenda]
# Google Agenda > Configurações > (sua agenda) > "Endereço secreto no formato iCal"
urls = []
avisar_minutos = [10, 0]   # avisos piscando: 10 min antes e na hora (0)
atualizar_minutos = 10

[claude]
# Sessões do Claude Code (item "sessoes") e limites de uso do plano (item "limites").
avisar_limite = [80, 95]     # pisca um aviso quando o uso de 5 h ou 7 dias passa destes %
piscar_permissao = true      # pedido de permissão/pergunta do Claude pisca no painel
esquecer_horas = 12          # some da lista a sessão sem sinal há mais que isso
detalhar = true              # ao terminar, mostra o resumo do que o Claude fez (senão, só "terminou")
"##;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub painel: PanelCfg,
    pub linha: Vec<LineCfg>,
    pub agenda: AgendaCfg,
    pub claude: ClaudeCfg,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PanelCfg {
    pub largura: String,
    pub linhas: usize,
    pub tamanho_led: f64,
    pub posicao: Position,
    pub velocidade: f32,
    pub brilho: u8,
    pub linha_mensagens: usize,
    /// 0 = automático (fica onde for arrastado); 1, 2... = monitor fixo (da esquerda para a direita).
    pub monitor: usize,
    /// Com posição "topo" ou "base": reserva a faixa da tela, e janelas maximizadas não cobrem o painel.
    pub reservar_espaco: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Topo,
    Base,
    Livre,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct LineCfg {
    pub blocos: Vec<BlockCfg>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    #[default]
    Fixo,
    Rolar,
    Piscar,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct BlockCfg {
    pub itens: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub largura: Option<usize>,
    pub efeito: Effect,
    pub segundos: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ClaudeCfg {
    pub avisar_limite: Vec<u32>,
    pub piscar_permissao: bool,
    pub esquecer_horas: u64,
    pub detalhar: bool,
}

impl Default for ClaudeCfg {
    fn default() -> Self {
        Self { avisar_limite: vec![80, 95], piscar_permissao: true, esquecer_horas: 12, detalhar: true }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct AgendaCfg {
    pub urls: Vec<String>,
    pub avisar_minutos: Vec<i64>,
    pub atualizar_minutos: u64,
}

impl Default for Config {
    fn default() -> Self {
        let items = ["data", "sistema", "rede", "historico", "agenda", "sessoes", "limites"];
        Self {
            painel: PanelCfg::default(),
            linha: vec![LineCfg {
                blocos: vec![
                    BlockCfg { itens: vec!["relogio".into()], largura: Some(31), ..Default::default() },
                    BlockCfg { itens: items.map(String::from).to_vec(), ..Default::default() },
                ],
            }],
            agenda: AgendaCfg::default(),
            claude: ClaudeCfg::default(),
        }
    }
}

impl Default for PanelCfg {
    fn default() -> Self {
        Self {
            largura: "60%".into(),
            linhas: 1,
            tamanho_led: 8.0,
            posicao: Position::Livre,
            velocidade: 28.0,
            brilho: 100,
            linha_mensagens: 1,
            monitor: 0,
            reservar_espaco: false,
        }
    }
}

impl Default for BlockCfg {
    fn default() -> Self {
        Self { itens: vec![], largura: None, efeito: Effect::Fixo, segundos: 5.0, cor: None }
    }
}

impl Default for AgendaCfg {
    fn default() -> Self {
        Self { urls: vec![], avisar_minutos: vec![10, 0], atualizar_minutos: 10 }
    }
}

pub enum Width {
    Percent(f64),
    Pixels(f64),
}

impl PanelCfg {
    pub fn width(&self) -> Width {
        let s = self.largura.trim();
        if let Some(p) = s.strip_suffix('%') {
            Width::Percent(p.trim().parse::<f64>().unwrap_or(60.0).clamp(5.0, 100.0))
        } else {
            let px = s.trim_end_matches("px").trim().parse::<f64>().unwrap_or(900.0);
            Width::Pixels(px.max(100.0))
        }
    }

    pub fn lines(&self) -> usize {
        self.linhas.clamp(1, 8)
    }

    /// Reserva a faixa da tela só faz sentido encostado no topo ou na base.
    pub fn reserves_space(&self) -> bool {
        self.reservar_espaco && matches!(self.posicao, Position::Topo | Position::Base)
    }

    pub fn message_line(&self) -> usize {
        self.linha_mensagens.clamp(1, self.lines()) - 1
    }
}

pub fn dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("painel-led")
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

/// Garante que o arquivo exista (cria com o modelo comentado).
pub fn ensure_file() {
    let p = path();
    if !p.exists() {
        let _ = std::fs::create_dir_all(dir());
        let _ = std::fs::write(&p, DEFAULT_TOML);
    }
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()).ok()?.modified().ok()
}

pub fn load() -> Result<Config, String> {
    let text = std::fs::read_to_string(path()).map_err(|e| e.to_string())?;
    toml::from_str(&text).map_err(|e| {
        // Só a primeira linha do erro cabe no painel.
        e.message().lines().next().unwrap_or("erro de sintaxe").to_string()
    })
}

/// Grava a configuração vinda da tela de configurações (o painel recarrega sozinho).
pub fn save(cfg: &Config) -> Result<(), String> {
    let body = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    // Garante que o que gravamos volta a ser lido sem erro.
    toml::from_str::<Config>(&body).map_err(|e| e.to_string())?;
    let header = "# Painel LED: configuração (gerada pela tela de configurações)\n\
                  # Pode editar à mão; salve e o painel recarrega sozinho.\n\n";
    let _ = std::fs::create_dir_all(dir());
    std::fs::write(path(), format!("{header}{body}")).map_err(|e| e.to_string())
}

pub fn restore_default() -> Result<(), String> {
    let _ = std::fs::create_dir_all(dir());
    std::fs::write(path(), DEFAULT_TOML).map_err(|e| e.to_string())
}

/// Abre o arquivo de configuração no editor de texto do sistema.
pub fn open_in_editor() {
    let p = path();
    #[cfg(windows)]
    let _ = std::process::Command::new("notepad.exe").arg(&p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-t").arg(&p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(&p).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_toml_parses() {
        let c: Config = toml::from_str(DEFAULT_TOML).unwrap();
        assert_eq!(c.linha.len(), 1);
        assert_eq!(c.linha[0].blocos[0].largura, Some(31));
        assert_eq!(c.agenda.avisar_minutos, vec![10, 0]);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let c: Config = toml::from_str("[painel]\nlinhas = 2\n").unwrap();
        assert_eq!(c.painel.lines(), 2);
        assert_eq!(c.linha.len(), 1);
    }
}
