//! Estado compartilhado entre a janela e as threads coletoras.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::config::{AgendaCfg, ClaudeCfg, Effect};

pub type Shared = Arc<Mutex<State>>;
pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const GREEN: Rgb = Rgb(60, 255, 90);
    pub const AMBER: Rgb = Rgb(255, 176, 0);
    pub const RED: Rgb = Rgb(255, 50, 40);
    pub const CYAN: Rgb = Rgb(0, 220, 255);
    pub const WHITE: Rgb = Rgb(235, 235, 235);
    pub const ORANGE: Rgb = Rgb(255, 120, 20);
    pub const MAGENTA: Rgb = Rgb(255, 60, 220);
    pub const BLUE: Rgb = Rgb(60, 120, 255);
    pub const YELLOW: Rgb = Rgb(255, 230, 40);
    pub const DIM: Rgb = Rgb(70, 70, 70);

    /// Aceita "#rrggbb", "rrggbb" ou nomes (em português ou inglês).
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim().to_lowercase();
        let named = match s.as_str() {
            "green" | "verde" => Some(Rgb::GREEN),
            "amber" | "ambar" | "âmbar" => Some(Rgb::AMBER),
            "red" | "vermelho" => Some(Rgb::RED),
            "cyan" | "ciano" => Some(Rgb::CYAN),
            "white" | "branco" => Some(Rgb::WHITE),
            "orange" | "laranja" => Some(Rgb::ORANGE),
            "magenta" | "rosa" | "pink" => Some(Rgb::MAGENTA),
            "blue" | "azul" => Some(Rgb::BLUE),
            "yellow" | "amarelo" => Some(Rgb::YELLOW),
            _ => None,
        };
        if named.is_some() {
            return named;
        }
        let hex = s.trim_start_matches('#');
        if hex.len() == 6 {
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        None
    }
}

/// Mensagem externa (API, Claude Code, avisos da agenda) que ocupa a linha de mensagens.
#[derive(Clone, Debug)]
pub struct Message {
    pub text: String,
    pub color: Rgb,
    pub effect: Effect,
    /// Para textos que rolam: quantas vezes passam pelo painel.
    pub repeat: u32,
    /// Para textos fixos/piscando que cabem no painel: quanto tempo ficam.
    pub seconds: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PingSample {
    /// Unix timestamp (segundos).
    pub t: i64,
    /// Latência em ms; `None` = offline.
    pub ms: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub title: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
}

/// Situação de uma sessão do Claude Code, segundo os hooks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Aberta, sem atividade ainda.
    Aberta,
    Trabalhando,
    /// Pedindo permissão ou fazendo uma pergunta: precisa de você agora.
    PrecisaDeVoce,
    /// Terminou e está esperando há um tempo (lembrete do Claude Code).
    Esperando,
    Concluida,
}

#[derive(Clone, Debug)]
pub struct ClaudeSession {
    pub id: String,
    pub project: String,
    pub status: SessionStatus,
    /// Desde quando está nesse estado.
    pub since: DateTime<Local>,
    /// Último sinal recebido.
    pub seen: DateTime<Local>,
    pub detail: Option<String>,
    pub model: Option<String>,
    pub context_pct: Option<f32>,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LimitWindow {
    /// 0 a 100.
    pub pct: f32,
    /// Unix timestamp (segundos) em que a janela renova.
    pub resets_at: i64,
}

/// Limites de uso do plano (vêm da statusline do Claude Code).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Limits {
    pub five_hour: Option<LimitWindow>,
    pub seven_day: Option<LimitWindow>,
    /// Unix timestamp da última atualização (0 = nunca).
    pub updated: i64,
}

#[derive(Default)]
pub struct State {
    pub cpu: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    pub pings: VecDeque<PingSample>,
    pub messages: VecDeque<Message>,
    /// Textos nomeados definidos via API (itens "var:nome").
    pub vars: HashMap<String, (String, Option<Rgb>)>,
    pub agenda_cfg: AgendaCfg,
    pub events: Vec<Event>,
    pub agenda_error: Option<String>,
    /// Mensagens agendadas (lembretes): entram na fila quando chega a hora.
    pub scheduled: Vec<(DateTime<Local>, Message)>,
    pub claude_cfg: ClaudeCfg,
    /// Sessões do Claude Code, na ordem em que apareceram.
    pub sessions: Vec<ClaudeSession>,
    pub limits: Limits,
    /// Quantos limiares de aviso já foram avisados em cada janela (5 h, 7 dias).
    pub limit_alerted: [usize; 2],
}

impl State {
    pub fn last_ping(&self) -> Option<PingSample> {
        self.pings.back().copied()
    }

    /// Move para a fila os lembretes cujo horário chegou.
    pub fn release_due(&mut self) {
        let now = Local::now();
        let (due, later): (Vec<_>, Vec<_>) = self.scheduled.drain(..).partition(|(t, _)| *t <= now);
        self.scheduled = later;
        for (_, m) in due {
            self.push_message(m);
        }
    }

    /// Alertas (piscar) furam a fila.
    pub fn push_message(&mut self, m: Message) {
        const MAX_QUEUE: usize = 20;
        if self.messages.len() >= MAX_QUEUE {
            self.messages.pop_back();
        }
        if m.effect == Effect::Piscar {
            self.messages.push_front(m);
        } else {
            self.messages.push_back(m);
        }
    }
}
