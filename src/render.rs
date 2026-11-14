//! Transforma configuração + estado em uma grade de LEDs (linhas x colunas).

use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, Timelike};

use crate::config::{BlockCfg, Config, Effect};
use crate::font;
use crate::state::{Message, Rgb, SessionStatus, State};

/// Uma coluna de uma linha de texto: bits das 7 linhas (bit 0 = topo) e a cor.
pub type Col = (u8, Rgb);
pub const BLANK: Col = (0, Rgb::DIM);
const DIVIDER: Col = (0x7F, Rgb(40, 40, 40));

pub struct Frame {
    pub lines: Vec<Vec<Col>>,
    /// Cor da moldura quando há mensagem ativa.
    pub border: Option<Rgb>,
    pub next: Duration,
}

fn text(s: &str, color: Rgb) -> Vec<Col> {
    font::text_columns(s).into_iter().map(|b| (b, color)).collect()
}

fn gap(n: usize) -> Vec<Col> {
    vec![BLANK; n]
}

/// Fase do pisca-pisca: aceso 60% do tempo.
fn blink_on(epoch: Instant) -> bool {
    epoch.elapsed().as_millis() % 1000 < 600
}

// ---------------------------------------------------------------- itens

pub fn weekday_pt(d: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match d {
        Mon => "SEG",
        Tue => "TER",
        Wed => "QUA",
        Thu => "QUI",
        Fri => "SEX",
        Sat => "SÁB",
        Sun => "DOM",
    }
}

fn load_color(pct: f32) -> Rgb {
    if pct < 50.0 {
        Rgb::GREEN
    } else if pct < 80.0 {
        Rgb::AMBER
    } else {
        Rgb::RED
    }
}

fn clock(now: DateTime<Local>, color: Rgb) -> Vec<Col> {
    let mut c = text(&format!("{:02}", now.hour()), color);
    c.push(BLANK);
    c.push(if now.second() % 2 == 0 { (0x36, color) } else { BLANK });
    c.push(BLANK);
    c.extend(text(&format!("{:02}", now.minute()), color));
    c
}

fn system(s: &State, cpu: bool, ram: bool) -> Vec<Col> {
    let ram_pct = if s.ram_total_gb > 0.0 { s.ram_used_gb / s.ram_total_gb * 100.0 } else { 0.0 };
    let mut out = Vec::new();
    if cpu {
        out.extend(text("CPU ", Rgb::CYAN));
        out.extend(text(&format!("{:.0}%", s.cpu), load_color(s.cpu)));
    }
    if cpu && ram {
        out.extend(gap(5));
    }
    if ram {
        out.extend(text("RAM ", Rgb::CYAN));
        out.extend(text(&format!("{:.0}%", ram_pct), load_color(ram_pct)));
    }
    out
}

fn network(s: &State) -> Vec<Col> {
    let mut net = text("NET ", Rgb::CYAN);
    match s.last_ping() {
        Some(p) => match p.ms {
            Some(ms) => {
                let c = if ms < 80 { Rgb::GREEN } else if ms < 200 { Rgb::AMBER } else { Rgb::RED };
                net.extend(text(&format!("{ms}ms"), c));
            }
            None => net.extend(text("OFFLINE", Rgb::RED)),
        },
        None => net.extend(text("...", Rgb::DIM)),
    }
    net
}

/// "24H" + 48 colunas (blocos de 30 min): verde = ok, âmbar = falhas parciais, vermelho = offline.
fn history(s: &State) -> Vec<Col> {
    const BUCKETS: i64 = 48;
    const BUCKET_SECS: i64 = 1800;
    let now = chrono::Utc::now().timestamp();
    let mut ok = [0u32; BUCKETS as usize];
    let mut fail = [0u32; BUCKETS as usize];
    for p in &s.pings {
        let age = now - p.t;
        if !(0..BUCKETS * BUCKET_SECS).contains(&age) {
            continue;
        }
        let idx = (BUCKETS - 1 - age / BUCKET_SECS) as usize;
        if p.ms.is_some() { ok[idx] += 1 } else { fail[idx] += 1 }
    }
    let mut cols = text("24H", Rgb::CYAN);
    cols.extend(gap(2));
    for i in 0..BUCKETS as usize {
        cols.push(match (ok[i], fail[i]) {
            (0, 0) => (0x40, Rgb::DIM),
            (_, 0) => (0x7C, Rgb::GREEN),
            (0, _) => (0x7F, Rgb::RED),
            _ => (0x7E, Rgb::AMBER),
        });
    }
    cols
}

/// Próximos compromissos: até 3 entradas.
fn agenda(s: &State, now: DateTime<Local>) -> Vec<Vec<Col>> {
    if s.agenda_cfg.urls.is_empty() {
        return vec![];
    }
    if s.events.is_empty() {
        return vec![match &s.agenda_error {
            Some(_) => text("AGENDA: ERRO", Rgb::RED),
            None => text("AGENDA LIVRE", Rgb::GREEN),
        }];
    }
    let today = now.date_naive();
    let mut out = Vec::new();
    for e in &s.events {
        if out.len() >= 3 {
            break;
        }
        let day = e.start.date_naive();
        let entry = if e.all_day {
            if day > today {
                continue;
            }
            let mut c = text("HOJE: ", Rgb::GREEN);
            c.extend(text(&e.title, Rgb::WHITE));
            c
        } else if e.start <= now {
            let mut c = text("AGORA: ", Rgb::AMBER);
            c.extend(text(&e.title, Rgb::WHITE));
            c
        } else {
            let when = if day == today {
                e.start.format("%H:%M").to_string()
            } else if day == today.succ_opt().unwrap_or(today) {
                format!("AMANHÃ {}", e.start.format("%H:%M"))
            } else {
                e.start.format("%d/%m %H:%M").to_string()
            };
            let mut c = text(&when, Rgb::CYAN);
            c.extend(gap(4));
            c.extend(text(&e.title, Rgb::WHITE));
            c
        };
        out.push(entry);
    }
    out
}

// Ícones de status (colunas de 7 bits).
const ICON_PLAY: [u8; 4] = [0x7F, 0x3E, 0x1C, 0x08];
const ICON_CHECK: [u8; 6] = [0x10, 0x20, 0x40, 0x20, 0x10, 0x08];
const ICON_BANG: [u8; 3] = [0x00, 0x5F, 0x00];
const ICON_DOT: [u8; 3] = [0x1C, 0x1C, 0x1C];
const ICON_WAIT: [u8; 5] = [0x40, 0x00, 0x40, 0x00, 0x40];

fn icon(bits: &[u8], color: Rgb) -> Vec<Col> {
    bits.iter().map(|&b| (b, color)).collect()
}

/// Sessões do Claude Code: "api ▶  web !  docs ✓". A que precisa de você pisca.
fn sessions(s: &State, on: bool, blinking: &mut bool) -> Vec<Vec<Col>> {
    const LABEL: Rgb = Rgb(0, 150, 180);
    if s.sessions.is_empty() {
        let mut out = text("CLAUDE ", LABEL);
        out.extend(text("sem sessões", Rgb(110, 110, 110)));
        return vec![out];
    }
    // "CLAUDE" na frente deixa claro que a lista é das sessões do Claude Code.
    let mut out = text("CLAUDE ", LABEL);
    for (i, x) in s.sessions.iter().enumerate() {
        if i > 0 {
            out.extend(gap(6));
        }
        let (color, ic): (Rgb, &[u8]) = match x.status {
            SessionStatus::Trabalhando => (Rgb::CYAN, &ICON_PLAY),
            SessionStatus::PrecisaDeVoce => (Rgb::AMBER, &ICON_BANG),
            SessionStatus::Esperando => (Rgb::AMBER, &ICON_WAIT),
            SessionStatus::Concluida => (Rgb::GREEN, &ICON_CHECK),
            SessionStatus::Aberta => (Rgb(130, 130, 130), &ICON_DOT),
        };
        let name: String = if x.project.chars().count() > 16 {
            format!("{}..", x.project.chars().take(14).collect::<String>())
        } else {
            x.project.clone()
        };
        let mut entry = text(&name, color);
        entry.push(BLANK);
        entry.extend(icon(ic, color));
        if x.status == SessionStatus::PrecisaDeVoce {
            *blinking = true;
            if !on {
                entry = vec![BLANK; entry.len()];
            }
        }
        out.extend(entry);
    }
    vec![out]
}

fn limit_color(pct: f32) -> Rgb {
    if pct < 60.0 {
        Rgb::GREEN
    } else if pct < 85.0 {
        Rgb::AMBER
    } else {
        Rgb::RED
    }
}

/// Limites do plano: "5H 23% 14:30   7D 41% SEX 10H".
fn limits(s: &State, width: usize) -> Vec<Vec<Col>> {
    let l = &s.limits;
    if l.five_hour.is_none() && l.seven_day.is_none() {
        return vec![];
    }
    // Do mais completo ao mais compacto; usa o primeiro que cabe no bloco.
    let variants = [(true, true), (false, true), (false, false)];
    let mut last = vec![];
    for (bar, time) in variants {
        last = limits_variant(l, bar, time);
        if last.len() <= width {
            break;
        }
    }
    vec![last]
}

fn limits_variant(l: &crate::state::Limits, bar: bool, time: bool) -> Vec<Col> {
    let mut out = Vec::new();
    for (label, w) in [("5H ", l.five_hour), ("7D ", l.seven_day)] {
        let Some(w) = w else { continue };
        if !out.is_empty() {
            out.extend(gap(6));
        }
        out.extend(text(label, Rgb::CYAN));
        let c = limit_color(w.pct);
        if bar {
            // Barrinha de 5 segmentos (cada um = 20%).
            let filled = ((w.pct / 20.0).round() as usize).min(5);
            for i in 0..5 {
                let seg = if i < filled { (0x7E, c) } else { (0x60, Rgb(55, 55, 55)) };
                out.extend([seg, seg, BLANK]);
            }
            out.push(BLANK);
        }
        out.extend(text(&format!("{:.0}%", w.pct), c));
        // Horário de renovação só quando está perto (menos de 20 h).
        if !time {
            continue;
        }
        if let Some(t) = chrono::TimeZone::timestamp_opt(&Local, w.resets_at, 0).single() {
            if w.resets_at > 0 && t - Local::now() < chrono::Duration::hours(20) {
                out.extend(gap(3));
                out.extend(text(&t.format("%H:%M").to_string(), Rgb(130, 130, 130)));
            }
        }
    }
    out
}

/// Cada item vira zero ou mais "entradas" (a agenda, por exemplo, gera uma por compromisso).
fn item_entries(
    item: &str,
    color: Option<Rgb>,
    s: &State,
    now: DateTime<Local>,
    on: bool,
    blinking: &mut bool,
    width: usize,
) -> Vec<Vec<Col>> {
    let item = item.trim();
    if let Some(t) = item.strip_prefix("texto:").or_else(|| item.strip_prefix("text:")) {
        return vec![text(t, color.unwrap_or(Rgb::AMBER))];
    }
    if let Some(name) = item.strip_prefix("var:") {
        return match s.vars.get(name) {
            Some((t, c)) => vec![text(t, c.or(color).unwrap_or(Rgb::WHITE))],
            None => vec![],
        };
    }
    match item {
        "relogio" | "relógio" | "clock" => vec![clock(now, color.unwrap_or(Rgb::AMBER))],
        "data" | "date" => vec![text(
            &format!("{} {:02}/{:02}", weekday_pt(now.weekday()), now.day(), now.month()),
            color.unwrap_or(Rgb::WHITE),
        )],
        "sistema" | "system" => vec![system(s, true, true)],
        "cpu" => vec![system(s, true, false)],
        "ram" => vec![system(s, false, true)],
        "rede" | "net" => vec![network(s)],
        "historico" | "histórico" | "history" => vec![history(s)],
        "agenda" | "calendar" => agenda(s, now),
        "sessoes" | "sessões" | "sessions" | "claude" => sessions(s, on, blinking),
        "limites" | "limite" | "limits" | "uso" => limits(s, width),
        other => vec![text(&format!("?{other}"), Rgb::RED)],
    }
}

// ---------------------------------------------------------------- blocos

#[derive(Clone)]
struct BlockState {
    idx: usize,
    started: Instant,
}

struct ActiveMessage {
    msg: Message,
    started: Instant,
}

pub struct Composer {
    blocks: Vec<Vec<BlockState>>,
    active: Option<ActiveMessage>,
    epoch: Instant,
}

/// Copia `src` para `dst` começando em `x0`, limitado a [lo, hi). Com `visible = false`, não desenha.
fn blit(dst: &mut [Col], src: &[Col], x0: i32, lo: usize, hi: usize, visible: bool) {
    if !visible {
        return;
    }
    for (i, c) in src.iter().enumerate() {
        let x = x0 + i as i32;
        if x >= lo as i32 && x < hi as i32 {
            dst[x as usize] = *c;
        }
    }
}

/// Divide a largura da linha entre os blocos (os sem `largura` dividem o que sobra).
fn block_ranges(blocks: &[BlockCfg], cols: usize) -> Vec<(usize, usize)> {
    let fixed: usize = blocks.iter().filter_map(|b| b.largura).sum();
    let flex = blocks.iter().filter(|b| b.largura.is_none()).count();
    let flex_w = if flex > 0 { cols.saturating_sub(fixed) / flex } else { 0 };
    let mut x = 0;
    let mut out = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let mut w = b.largura.unwrap_or(flex_w);
        if i == blocks.len() - 1 && b.largura.is_none() {
            w = cols.saturating_sub(x); // o último flexível absorve o arredondamento
        }
        let w = w.min(cols.saturating_sub(x));
        out.push((x, x + w));
        x += w;
    }
    out
}

impl Composer {
    pub fn new() -> Self {
        Self { blocks: vec![], active: None, epoch: Instant::now() }
    }

    /// Recomeça as rotações (após recarregar a configuração).
    pub fn reset(&mut self) {
        self.blocks.clear();
    }

    pub fn skip_message(&mut self) {
        self.active = None;
    }

    pub fn compose(&mut self, cfg: &Config, s: &mut State, cols: usize) -> Frame {
        let n_lines = cfg.painel.lines();
        let speed = cfg.painel.velocidade.clamp(2.0, 200.0);
        let now = Local::now();
        let mut lines = vec![vec![BLANK; cols]; n_lines];
        let mut animating = false;
        let mut border = None;

        if self.blocks.len() != n_lines {
            self.blocks = vec![vec![]; n_lines];
        }

        // Mensagem ativa (ocupa a linha de mensagens inteira).
        s.release_due();
        crate::claude::prune(s);
        crate::claude::expire_limits(s);
        let on = blink_on(self.epoch);
        let msg_line = cfg.painel.message_line();
        // Alerta (piscar) interrompe uma mensagem comum; ela volta depois do alerta.
        let urgent = s.messages.front().is_some_and(|m| m.effect == Effect::Piscar);
        if urgent && self.active.as_ref().is_some_and(|a| a.msg.effect != Effect::Piscar) {
            let alert = s.messages.pop_front().unwrap();
            let old = self.active.take().unwrap().msg;
            s.messages.push_front(old);
            self.active = Some(ActiveMessage { msg: alert, started: Instant::now() });
        }
        if self.active.is_none() {
            if let Some(msg) = s.messages.pop_front() {
                self.active = Some(ActiveMessage { msg, started: Instant::now() });
            }
        }
        if let Some(a) = &self.active {
            let m = &a.msg;
            let content = text(&m.text, m.color);
            let elapsed = a.started.elapsed().as_secs_f32();
            let fits = content.len() <= cols;
            let blink = m.effect == Effect::Piscar;
            let visible = !blink || blink_on(self.epoch);
            let line = &mut lines[msg_line];
            let done = if fits && m.effect != Effect::Rolar {
                let x0 = ((cols - content.len()) / 2) as i32;
                blit(line, &content, x0, 0, cols, visible);
                elapsed >= m.seconds
            } else {
                let travel = (cols + content.len()) as f32;
                let pos = elapsed * speed;
                blit(line, &content, cols as i32 - (pos % travel) as i32, 0, cols, visible);
                pos >= travel * m.repeat as f32
            };
            if done {
                self.active = None;
                self.blocks[msg_line].clear();
                lines[msg_line] = vec![BLANK; cols];
            } else {
                animating = true;
                border = if !blink || blink_on(self.epoch) { Some(m.color) } else { None };
                if blink {
                    border = border.or(Some(Rgb(30, 30, 30)));
                }
            }
        }

        for (li, line_cfg) in cfg.linha.iter().take(n_lines).enumerate() {
            if self.active.is_some() && li == msg_line {
                continue;
            }
            let ranges = block_ranges(&line_cfg.blocos, cols);
            if self.blocks[li].len() != ranges.len() {
                self.blocks[li] = vec![BlockState { idx: 0, started: Instant::now() }; ranges.len()];
            }
            for (bi, (b, &(x0, x1))) in line_cfg.blocos.iter().zip(&ranges).enumerate() {
                if x1 <= x0 + 2 {
                    continue;
                }
                let last = bi == ranges.len() - 1;
                // Área útil: 1 coluna de respiro de cada lado; divisória entre blocos.
                let (lo, hi) = (x0 + 1, x1 - 1);
                if !last {
                    lines[li][x1 - 1] = DIVIDER;
                }
                let color = b.cor.as_deref().and_then(Rgb::parse);
                let mut blinking = false;
                let entries: Vec<Vec<Col>> =
                    b.itens.iter().flat_map(|it| item_entries(it, color, s, now, on, &mut blinking, hi - lo)).collect();
                if blinking {
                    animating = true;
                }
                if entries.is_empty() {
                    continue;
                }
                let st = &mut self.blocks[li][bi];
                let entry = &entries[st.idx % entries.len()];
                let w = hi - lo;
                let fits = entry.len() <= w;
                let elapsed = st.started.elapsed().as_secs_f32();
                let visible = b.efeito != Effect::Piscar || blink_on(self.epoch);
                if b.efeito == Effect::Piscar {
                    animating = true;
                }
                let duration = if fits && b.efeito != Effect::Rolar {
                    let x = lo + (w - entry.len()) / 2;
                    blit(&mut lines[li], entry, x as i32, lo, hi, visible);
                    b.segundos.max(1.0)
                } else {
                    animating = true;
                    // Texto rolando: entra pela direita e sai pela esquerda.
                    let travel = (w + entry.len()) as f32;
                    let pos = elapsed * speed;
                    let x = hi as i32 - (pos % travel) as i32;
                    blit(&mut lines[li], entry, x, lo, hi, visible);
                    travel / speed
                };
                if elapsed >= duration {
                    st.idx = (st.idx + 1) % entries.len();
                    st.started = Instant::now();
                }
            }
        }

        let next = if animating {
            Duration::from_millis(33)
        } else {
            // Próxima virada de segundo (relógio piscando, troca de item).
            Duration::from_millis(1000 - (now.timestamp_subsec_millis() as u64 % 1000) + 5)
        };
        Frame { lines, border, next }
    }
}
