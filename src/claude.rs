//! Sessões do Claude Code (via hooks) e limites de uso do plano (via statusline).

use std::path::PathBuf;

use chrono::{Local, TimeZone};
use serde_json::{Value, json};

use crate::config::Effect;
use crate::state::{ClaudeSession, LimitWindow, Limits, Message, Rgb, SessionStatus, State};

const MAX_SESSIONS: usize = 12;

fn s<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.get(*k))
}

fn str_at(v: &Value, path: &[&str]) -> Option<String> {
    s(v, path).and_then(|x| x.as_str()).map(str::to_string).filter(|x| !x.is_empty())
}

fn f64_at(v: &Value, path: &[&str]) -> Option<f64> {
    s(v, path).and_then(|x| x.as_f64())
}

pub fn project_name(cwd: &str) -> String {
    let name = cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("");
    if name.is_empty() { "claude".into() } else { name.to_string() }
}

fn clean(text: &str) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(200).collect()
}

/// Encontra (ou cria) a sessão. Sem `session_id`, usa a pasta como identificador.
fn session<'a>(st: &'a mut State, id: &str, cwd: Option<&str>) -> &'a mut ClaudeSession {
    let now = Local::now();
    let idx = match st.sessions.iter().position(|x| x.id == id) {
        Some(i) => i,
        None => {
            if st.sessions.len() >= MAX_SESSIONS {
                // Descarta a mais antiga sem sinal.
                if let Some((i, _)) = st.sessions.iter().enumerate().min_by_key(|(_, x)| x.seen) {
                    st.sessions.remove(i);
                }
            }
            st.sessions.push(ClaudeSession {
                id: id.to_string(),
                project: project_name(cwd.unwrap_or("")),
                status: SessionStatus::Aberta,
                since: now,
                seen: now,
                detail: None,
                model: None,
                context_pct: None,
                cost_usd: None,
            });
            st.sessions.len() - 1
        }
    };
    let sess = &mut st.sessions[idx];
    sess.seen = now;
    if let Some(c) = cwd {
        sess.project = project_name(c);
    }
    sess
}

fn set_status(sess: &mut ClaudeSession, status: SessionStatus, detail: Option<String>) {
    if sess.status != status {
        sess.since = Local::now();
    }
    sess.status = status;
    sess.detail = detail;
}

/// Remove sessões sem sinal há mais de `esquecer_horas`.
pub fn prune(st: &mut State) {
    let hours = st.claude_cfg.esquecer_horas.max(1) as i64;
    let limit = Local::now() - chrono::Duration::hours(hours);
    st.sessions.retain(|x| x.seen >= limit);
}

/// "Claude needs your permission to use Bash" → "permissão para Bash".
fn permission_text(msg: &str) -> String {
    let lower = msg.to_lowercase();
    match lower.find("permission to use ") {
        Some(i) => format!("pede permissão para {}", msg[i + "permission to use ".len()..].trim()),
        None => "pede sua permissão".into(),
    }
}

/// Processa o JSON de um hook do Claude Code: atualiza a sessão e devolve a mensagem para o painel, se houver.
pub fn handle_hook(st: &mut State, body: &str) -> Option<Message> {
    let v: Value = serde_json::from_str(body).ok()?;
    let event = str_at(&v, &["hook_event_name"])?;
    let cwd = str_at(&v, &["cwd"]);
    let id = str_at(&v, &["session_id"]).or_else(|| cwd.clone()).unwrap_or_else(|| "claude".into());
    let blink = st.claude_cfg.piscar_permissao;

    if event == "SessionEnd" {
        st.sessions.retain(|x| x.id != id);
        return None;
    }
    let sess = session(st, &id, cwd.as_deref());
    let prefix = format!("CLAUDE [{}]: ", sess.project);
    let msg = |text: String, color: Rgb, effect: Effect| Message {
        text: clean(&format!("{prefix}{text}")),
        color,
        effect,
        repeat: if effect == Effect::Piscar { 3 } else { 2 },
        seconds: if effect == Effect::Piscar { 15.0 } else { 8.0 },
    };

    match event.as_str() {
        "SessionStart" => {
            if sess.status != SessionStatus::Trabalhando {
                set_status(sess, SessionStatus::Aberta, None);
            }
            None
        }
        "UserPromptSubmit" | "PostToolUse" | "PreToolUse" => {
            set_status(sess, SessionStatus::Trabalhando, None);
            None
        }
        "Stop" => {
            set_status(sess, SessionStatus::Concluida, None);
            Some(msg("terminou - sua vez!".into(), Rgb::GREEN, Effect::Rolar))
        }
        "StopFailure" => {
            let why = str_at(&v, &["error"]).or_else(|| str_at(&v, &["message"]));
            set_status(sess, SessionStatus::Concluida, why.clone());
            let text = match why {
                Some(w) => format!("parou com erro: {w}"),
                None => "parou com erro".into(),
            };
            Some(msg(text, Rgb::RED, Effect::Piscar))
        }
        "Notification" => {
            let message = str_at(&v, &["message"]).unwrap_or_default();
            let kind = str_at(&v, &["notification_type"]).unwrap_or_else(|| {
                let m = message.to_lowercase();
                if m.contains("permission") {
                    "permission_prompt".into()
                } else if m.contains("waiting for your input") {
                    "idle_prompt".into()
                } else {
                    String::new()
                }
            });
            let effect = if blink { Effect::Piscar } else { Effect::Rolar };
            match kind.as_str() {
                "permission_prompt" => {
                    let text = permission_text(&message);
                    set_status(sess, SessionStatus::PrecisaDeVoce, Some(text.clone()));
                    Some(msg(text, Rgb::AMBER, effect))
                }
                "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
                    set_status(sess, SessionStatus::PrecisaDeVoce, Some("tem uma pergunta".into()));
                    Some(msg("tem uma pergunta para você".into(), Rgb::AMBER, effect))
                }
                "idle_prompt" => {
                    set_status(sess, SessionStatus::Esperando, None);
                    Some(msg("esperando sua resposta".into(), Rgb::AMBER, Effect::Rolar))
                }
                "auth_success" | "elicitation_complete" | "elicitation_response" | "plugin_notification" => None,
                _ if message.is_empty() => None,
                _ => Some(msg(message, Rgb::AMBER, Effect::Rolar)),
            }
        }
        _ => None,
    }
}

fn window(v: &Value, key: &str) -> Option<LimitWindow> {
    let w = s(v, &["rate_limits", key])?;
    Some(LimitWindow {
        pct: f64_at(w, &["used_percentage"])? as f32,
        resets_at: w.get("resets_at").and_then(|x| x.as_i64()).unwrap_or(0),
    })
}

fn fmt_reset(ts: i64) -> String {
    let Some(t) = Local.timestamp_opt(ts, 0).single() else { return String::new() };
    if t - Local::now() < chrono::Duration::hours(20) {
        t.format("%H:%M").to_string()
    } else {
        format!("{} {}", crate::render::weekday_pt(chrono::Datelike::weekday(&t)), t.format("%Hh"))
    }
}

/// Quantos limiares (ordenados) o percentual já atingiu.
fn level(thresholds: &[u32], pct: f32) -> usize {
    thresholds.iter().filter(|&&t| pct >= t as f32).count()
}

/// Avisa (piscando) quando um limite passa de um limiar novo.
fn check_alerts(st: &mut State) {
    let mut th = st.claude_cfg.avisar_limite.clone();
    th.sort_unstable();
    let windows = [("5H", st.limits.five_hour), ("7 DIAS", st.limits.seven_day)];
    for (i, (name, w)) in windows.into_iter().enumerate() {
        let Some(w) = w else { continue };
        let lv = level(&th, w.pct);
        if lv > st.limit_alerted[i] {
            let color = if lv >= th.len() { Rgb::RED } else { Rgb::ORANGE };
            let text = format!("LIMITE {name} DO CLAUDE: {:.0}% USADO - RENOVA {}", w.pct, fmt_reset(w.resets_at));
            st.push_message(Message { text, color, effect: Effect::Piscar, repeat: 3, seconds: 15.0 });
        }
        st.limit_alerted[i] = lv;
    }
}

fn limits_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("painel-led").join("claude-limites.json"))
}

/// Carrega o último valor conhecido (para mostrar algo logo ao abrir o painel).
pub fn load_limits(st: &mut State) {
    let Some(text) = limits_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
    if let Ok(l) = serde_json::from_str::<Limits>(&text) {
        st.limits = l;
        expire_limits(st);
        let mut th = st.claude_cfg.avisar_limite.clone();
        th.sort_unstable();
        // Não repete avisos já dados antes de reabrir o painel.
        st.limit_alerted = [
            st.limits.five_hour.map_or(0, |w| level(&th, w.pct)),
            st.limits.seven_day.map_or(0, |w| level(&th, w.pct)),
        ];
    }
}

/// Janela que já renovou: o valor antigo não vale mais.
pub fn expire_limits(st: &mut State) {
    let now = chrono::Utc::now().timestamp();
    for w in [&mut st.limits.five_hour, &mut st.limits.seven_day] {
        if w.is_some_and(|x| x.resets_at > 0 && x.resets_at <= now) {
            *w = None;
        }
    }
}

/// Processa o JSON da statusline: limites, modelo, contexto e custo da sessão.
/// Devolve o texto que a statusline mostra no terminal e, se os limites mudaram, o que gravar em disco.
pub fn handle_statusline(st: &mut State, body: &str) -> (String, Option<Limits>) {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return (String::new(), None) };
    let cwd = str_at(&v, &["workspace", "current_dir"]).or_else(|| str_at(&v, &["cwd"]));
    let model = str_at(&v, &["model", "display_name"]);
    let ctx = f64_at(&v, &["context_window", "used_percentage"]).map(|x| x as f32);
    if let Some(id) = str_at(&v, &["session_id"]) {
        let sess = session(st, &id, cwd.as_deref());
        sess.model = model.clone().or(sess.model.take());
        sess.context_pct = ctx.or(sess.context_pct);
        sess.cost_usd = f64_at(&v, &["cost", "total_cost_usd"]).or(sess.cost_usd);
    }

    let mut changed = None;
    if s(&v, &["rate_limits"]).is_some() {
        let new = Limits {
            five_hour: window(&v, "five_hour"),
            seven_day: window(&v, "seven_day"),
            updated: chrono::Utc::now().timestamp(),
        };
        let differs = new.five_hour != st.limits.five_hour || new.seven_day != st.limits.seven_day;
        st.limits = new;
        if differs {
            changed = Some(new);
            check_alerts(st);
        }
    }

    // Linha mostrada no rodapé do Claude Code.
    let mut parts = vec![];
    if let Some(m) = model {
        parts.push(m);
    }
    if let Some(c) = ctx {
        parts.push(format!("contexto {c:.0}%"));
    }
    if let Some(w) = st.limits.five_hour {
        parts.push(format!("5h {:.0}% (renova {})", w.pct, fmt_reset(w.resets_at)));
    }
    if let Some(w) = st.limits.seven_day {
        parts.push(format!("7d {:.0}%", w.pct));
    }
    (parts.join(" · "), changed)
}

pub fn save_limits(l: &Limits) {
    let Some(path) = limits_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(l) {
        let _ = std::fs::write(path, text);
    }
}

/// Sessões e limites em JSON (rota GET /claude, ferramenta MCP e tela de configurações).
pub fn to_json(st: &State) -> Value {
    let iso = |ts: i64| Local.timestamp_opt(ts, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string());
    let win = |w: Option<LimitWindow>| w.map(|w| json!({ "usado_pct": w.pct, "renova_em": iso(w.resets_at) }));
    let sessions: Vec<Value> = st
        .sessions
        .iter()
        .map(|x| {
            json!({
                "projeto": x.project,
                "estado": x.status,
                "desde": x.since.format("%H:%M:%S").to_string(),
                "detalhe": x.detail,
                "modelo": x.model,
                "contexto_pct": x.context_pct,
                "custo_usd": x.cost_usd,
                "session_id": x.id,
            })
        })
        .collect();
    json!({
        "sessoes": sessions,
        "limites": {
            "cinco_horas": win(st.limits.five_hour),
            "sete_dias": win(st.limits.seven_day),
            "atualizado_em": (st.limits.updated > 0).then(|| iso(st.limits.updated)).flatten(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(event: &str, extra: &str) -> String {
        format!(r#"{{"hook_event_name":"{event}","session_id":"s1","cwd":"C:\\dev\\api"{extra}}}"#)
    }

    #[test]
    fn session_lifecycle() {
        let mut st = State::default();
        st.claude_cfg = Default::default();
        assert!(handle_hook(&mut st, &hook("SessionStart", "")).is_none());
        assert_eq!(st.sessions[0].project, "api");
        handle_hook(&mut st, &hook("UserPromptSubmit", ""));
        assert_eq!(st.sessions[0].status, SessionStatus::Trabalhando);
        let m = handle_hook(
            &mut st,
            &hook("Notification", r#","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash""#),
        )
        .unwrap();
        assert_eq!(st.sessions[0].status, SessionStatus::PrecisaDeVoce);
        assert_eq!(m.effect, Effect::Piscar);
        assert!(m.text.contains("Bash"), "{}", m.text);
        handle_hook(&mut st, &hook("PostToolUse", ""));
        assert_eq!(st.sessions[0].status, SessionStatus::Trabalhando);
        assert!(handle_hook(&mut st, &hook("Stop", "")).is_some());
        assert_eq!(st.sessions[0].status, SessionStatus::Concluida);
        handle_hook(&mut st, &hook("SessionEnd", ""));
        assert!(st.sessions.is_empty());
    }

    #[test]
    fn statusline_limits_and_alerts() {
        let mut st = State::default();
        st.claude_cfg = Default::default();
        let future = chrono::Utc::now().timestamp() + 3600;
        let body = |p: f32| {
            format!(
                r#"{{"session_id":"s1","cwd":"/x/web","model":{{"display_name":"Opus"}},"context_window":{{"used_percentage":34}},
                "rate_limits":{{"five_hour":{{"used_percentage":{p},"resets_at":{future}}},"seven_day":{{"used_percentage":41,"resets_at":{future}}}}}}}"#
            )
        };
        let (line, changed) = handle_statusline(&mut st, &body(23.0));
        assert!(changed.is_some());
        assert!(line.contains("5h 23%"), "{line}");
        assert!(st.messages.is_empty());
        handle_statusline(&mut st, &body(85.0));
        assert_eq!(st.messages.len(), 1);
        handle_statusline(&mut st, &body(86.0));
        assert_eq!(st.messages.len(), 1, "não repete o mesmo limiar");
        handle_statusline(&mut st, &body(96.0));
        assert_eq!(st.messages.len(), 2);
        assert_eq!(st.sessions[0].project, "web");
    }
}
