//! Threads que alimentam o painel: sistema, conectividade e servidor HTTP local.

use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sysinfo::System;
use tiny_http::{Header, Method, Response, Server};

use crate::config::Effect;
use crate::state::{Message, PingSample, Rgb, Shared, Wake};

pub const HTTP_ADDR: &str = "127.0.0.1:7878";
const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const HISTORY_SECS: i64 = 24 * 3600;
const MAX_TEXT: usize = 300;

// ---------------------------------------------------------------- sistema

pub fn spawn_system(state: Shared) {
    thread::spawn(move || {
        let Ok(mut sys) = System::new() else { return };
        loop {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            {
                let mut s = state.lock().unwrap();
                s.cpu = sys.global_cpu_usage();
                s.ram_used_gb = sys.used_memory() as f32 / 1_073_741_824.0;
                s.ram_total_gb = sys.total_memory() as f32 / 1_073_741_824.0;
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

// ---------------------------------------------------------------- conectividade

fn history_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("painel-led").join("pings.json"))
}

fn load_history() -> Vec<PingSample> {
    let Some(path) = history_path() else { return vec![] };
    let Ok(data) = std::fs::read_to_string(path) else { return vec![] };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_history(samples: &[PingSample]) {
    let Some(path) = history_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(samples) {
        let _ = std::fs::write(path, json);
    }
}

/// "Ping" leve via conexão TCP (não precisa de privilégios de administrador, ao contrário do ICMP).
fn probe() -> Option<u32> {
    let targets: [SocketAddr; 2] = [([1, 1, 1, 1], 443).into(), ([8, 8, 8, 8], 53).into()];
    for addr in targets {
        let start = Instant::now();
        if TcpStream::connect_timeout(&addr, PING_TIMEOUT).is_ok() {
            return Some(start.elapsed().as_millis().max(1) as u32);
        }
    }
    None
}

pub fn spawn_ping(state: Shared, wake: Wake) {
    thread::spawn(move || {
        let now = chrono::Utc::now().timestamp();
        {
            let mut s = state.lock().unwrap();
            s.pings = load_history()
                .into_iter()
                .filter(|p| now - p.t < HISTORY_SECS)
                .collect();
        }
        let mut since_save = 0;
        loop {
            let ms = probe();
            let t = chrono::Utc::now().timestamp();
            let snapshot = {
                let mut s = state.lock().unwrap();
                s.pings.push_back(PingSample { t, ms });
                while s.pings.front().is_some_and(|p| t - p.t >= HISTORY_SECS) {
                    s.pings.pop_front();
                }
                since_save += 1;
                (since_save >= 10).then(|| s.pings.iter().copied().collect::<Vec<_>>())
            };
            if let Some(samples) = snapshot {
                save_history(&samples);
                since_save = 0;
            }
            wake();
            thread::sleep(PING_INTERVAL);
        }
    });
}

// ---------------------------------------------------------------- HTTP

#[derive(Deserialize, Default)]
#[serde(default)]
struct MessageBody {
    #[serde(alias = "texto")]
    text: String,
    #[serde(alias = "cor")]
    color: Option<String>,
    #[serde(alias = "efeito")]
    effect: Option<String>,
    #[serde(alias = "repetir")]
    repeat: Option<u32>,
    #[serde(alias = "segundos")]
    seconds: Option<f32>,
    #[serde(alias = "nome")]
    name: Option<String>,
    /// Lembrete: mostra a mensagem daqui a N minutos.
    #[serde(alias = "in_minutes")]
    em_minutos: Option<f64>,
}

fn query_param(url: &str, keys: &[&str]) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        keys.contains(&k).then(|| percent_decode(&v.replace('+', " ")))
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn clean(text: &str) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(MAX_TEXT).collect()
}

fn parse_effect(s: &str) -> Option<Effect> {
    match s.trim().to_lowercase().as_str() {
        "fixo" | "fixed" | "static" => Some(Effect::Fixo),
        "rolar" | "scroll" => Some(Effect::Rolar),
        "piscar" | "blink" | "alerta" | "alert" => Some(Effect::Piscar),
        _ => None,
    }
}

/// Corpo em JSON ou texto puro; parâmetros também podem vir na URL.
fn parse_body(url: &str, body: &str) -> MessageBody {
    let mut m = serde_json::from_str::<MessageBody>(body)
        .unwrap_or_else(|_| MessageBody { text: body.to_string(), ..Default::default() });
    m.color = m.color.or_else(|| query_param(url, &["color", "cor"]));
    m.effect = m.effect.or_else(|| query_param(url, &["effect", "efeito"]));
    m.repeat = m.repeat.or_else(|| query_param(url, &["repeat", "repetir"])?.parse().ok());
    m.seconds = m.seconds.or_else(|| query_param(url, &["seconds", "segundos"])?.parse().ok());
    m.name = m.name.or_else(|| query_param(url, &["name", "nome"]));
    m.em_minutos = m.em_minutos.or_else(|| query_param(url, &["em_minutos", "in_minutes"])?.parse().ok());
    m.text = clean(&m.text);
    m
}

fn to_message(m: &MessageBody) -> Option<Message> {
    if m.text.is_empty() {
        return None;
    }
    let effect = m.effect.as_deref().and_then(parse_effect).unwrap_or(Effect::Rolar);
    let default_color = if effect == Effect::Piscar { Rgb::RED } else { Rgb::AMBER };
    Some(Message {
        text: m.text.clone(),
        color: m.color.as_deref().and_then(Rgb::parse).unwrap_or(default_color),
        effect,
        repeat: m.repeat.unwrap_or(if effect == Effect::Piscar { 3 } else { 2 }).clamp(1, 20),
        seconds: m.seconds.unwrap_or(if effect == Effect::Piscar { 10.0 } else { 8.0 }).clamp(1.0, 3600.0),
    })
}

pub fn bind_http() -> Option<Server> {
    Server::http(HTTP_ADDR).ok()
}

/// Token aleatório por execução: protege as rotas que alteram a configuração.
pub fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

const OK: &str = r#"{"ok":true}"#;
const CONFIG_HTML: &str = include_str!("ui/config.html");

fn header<'a>(req: &'a tiny_http::Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

/// Só aceita chamadas locais: bloqueia sites abertos no navegador (Origin) e DNS rebinding (Host).
fn is_local(req: &tiny_http::Request) -> bool {
    let host_ok = header(req, "Host").is_none_or(|h| {
        let h = h.to_ascii_lowercase();
        h == "127.0.0.1:7878" || h == "localhost:7878"
    });
    let origin_ok = header(req, "Origin").is_none_or(|o| {
        let o = o.to_ascii_lowercase();
        o == "http://127.0.0.1:7878" || o == "http://localhost:7878"
    });
    host_ok && origin_ok
}

fn err(msg: &str) -> String {
    serde_json::json!({ "ok": false, "error": msg }).to_string()
}

pub fn spawn_http(server: Server, state: Shared, wake: Wake, token: String) {
    thread::spawn(move || {
        let json = Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap();
        let html = Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
        let text = Header::from_bytes("Content-Type", "text/plain; charset=utf-8").unwrap();
        for mut req in server.incoming_requests() {
            if !is_local(&req) {
                let _ = req.respond(Response::from_string(err("origem não permitida")).with_status_code(403));
                continue;
            }
            let mut body = String::new();
            let _ = req.as_reader().take(256 * 1024).read_to_string(&mut body);
            let url = req.url().to_string();
            let path = url.split('?').next().unwrap_or("").to_string();
            let authed = header(&req, "X-Painel-Token") == Some(token.as_str());
            let mut content_type = json.clone();

            let (status, reply): (u16, String) = match (req.method(), path.as_str()) {
                // Hooks do Claude Code: atualizam a lista de sessões e, às vezes, mostram uma mensagem.
                (Method::Post, "/claude-hook") => {
                    let mut s = state.lock().unwrap();
                    if let Some(m) = crate::claude::handle_hook(&mut s, &body) {
                        s.push_message(m);
                    }
                    (200, OK.into())
                }
                // Statusline do Claude Code: limites de uso, modelo e contexto. A resposta é a linha do terminal.
                (Method::Post, "/claude-statusline") => {
                    let (line, changed) = crate::claude::handle_statusline(&mut state.lock().unwrap(), &body);
                    if let Some(l) = changed {
                        crate::claude::save_limits(&l);
                    }
                    content_type = text.clone();
                    (200, line)
                }
                (Method::Get, "/claude") => {
                    let mut v = crate::claude::to_json(&state.lock().unwrap());
                    v["ok"] = serde_json::json!(true);
                    (200, v.to_string())
                }
                (Method::Post, "/message" | "/mensagem") => {
                    let parsed = Some(parse_body(&url, &body));
                    let msg = parsed.as_ref().and_then(to_message);
                    let delay = parsed.as_ref().and_then(|m| m.em_minutos).filter(|d| *d > 0.0);
                    match msg {
                        Some(m) => {
                            let mut s = state.lock().unwrap();
                            match delay {
                                Some(min) => {
                                    let at = chrono::Local::now() + chrono::Duration::seconds((min * 60.0) as i64);
                                    s.scheduled.push((at, m));
                                    let when = at.format("%H:%M:%S").to_string();
                                    (200, serde_json::json!({"ok": true, "agendado_para": when}).to_string())
                                }
                                None => {
                                    s.push_message(m);
                                    (200, OK.into())
                                }
                            }
                        }
                        None => (400, err("mensagem vazia ou inválida")),
                    }
                }
                // Texto nomeado, exibido pelos itens "var:nome" da configuração. Corpo vazio apaga.
                (Method::Post, "/texto" | "/text") => {
                    let m = parse_body(&url, &body);
                    match m.name.filter(|n| !n.is_empty()) {
                        Some(name) => {
                            let mut s = state.lock().unwrap();
                            if m.text.is_empty() {
                                s.vars.remove(&name);
                            } else {
                                let color = m.color.as_deref().and_then(Rgb::parse);
                                s.vars.insert(name, (m.text, color));
                            }
                            (200, OK.into())
                        }
                        None => (400, err("informe ?nome=")),
                    }
                }
                (Method::Post, "/clear" | "/limpar") => {
                    let mut s = state.lock().unwrap();
                    s.messages.clear();
                    s.scheduled.clear();
                    (200, OK.into())
                }
                (Method::Get, "/status") => {
                    let s = state.lock().unwrap();
                    let last = s.last_ping();
                    let vars: std::collections::HashMap<_, _> =
                        s.vars.iter().map(|(k, v)| (k.clone(), v.0.clone())).collect();
                    let events: Vec<_> = s
                        .events
                        .iter()
                        .take(10)
                        .map(|e| {
                            serde_json::json!({
                                "titulo": e.title,
                                "inicio": e.start.format("%Y-%m-%d %H:%M").to_string(),
                                "dia_inteiro": e.all_day,
                            })
                        })
                        .collect();
                    let reply = serde_json::json!({
                        "cpu": s.cpu,
                        "ram_used_gb": s.ram_used_gb,
                        "ram_total_gb": s.ram_total_gb,
                        "online": last.map(|p| p.ms.is_some()),
                        "latency_ms": last.and_then(|p| p.ms),
                        "queued_messages": s.messages.len(),
                        "scheduled_messages": s.scheduled.len(),
                        "vars": vars,
                        "agenda_events": events,
                        "agenda_error": s.agenda_error,
                        "claude": crate::claude::to_json(&s),
                    });
                    (200, reply.to_string())
                }
                // ---- tela de configurações
                (Method::Get, "/" | "/config" | "/configuracoes") => {
                    content_type = html.clone();
                    (200, CONFIG_HTML.replace("__TOKEN__", &token))
                }
                (Method::Get, "/api/config") => {
                    let (cfg, error) = match crate::config::load() {
                        Ok(c) => (c, None),
                        Err(e) => (crate::config::Config::default(), Some(e)),
                    };
                    let reply = serde_json::json!({
                        "ok": true,
                        "config": cfg,
                        "erro_no_arquivo": error,
                        "arquivo": crate::config::path().display().to_string(),
                    });
                    (200, reply.to_string())
                }
                (Method::Post, "/api/config") if authed => {
                    match serde_json::from_str::<crate::config::Config>(&body) {
                        Ok(cfg) => match crate::config::save(&cfg) {
                            Ok(()) => (200, OK.into()),
                            Err(e) => (500, err(&e)),
                        },
                        Err(e) => (400, err(&format!("configuração inválida: {e}"))),
                    }
                }
                (Method::Post, "/api/config/padrao") if authed => match crate::config::restore_default() {
                    Ok(()) => (200, OK.into()),
                    Err(e) => (500, err(&e)),
                },
                (Method::Post, "/api/config/arquivo") if authed => {
                    crate::config::open_in_editor();
                    (200, OK.into())
                }
                // ---- iniciar com o sistema
                (Method::Get, "/api/sistema") => {
                    let reply = serde_json::json!({
                        "ok": true,
                        "iniciar_com_sistema": crate::autostart::is_enabled(),
                        "sistema": std::env::consts::OS,
                        "executavel": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
                        "monitores": state.lock().unwrap().monitors.clone(),
                        "reservar_suportado": cfg!(any(windows, target_os = "linux")),
                    });
                    (200, reply.to_string())
                }
                (Method::Post, "/api/sistema") if authed => {
                    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    match v.get("iniciar_com_sistema").and_then(|b| b.as_bool()) {
                        Some(on) => match crate::autostart::set(on) {
                            Ok(()) => (200, serde_json::json!({"ok": true, "iniciar_com_sistema": crate::autostart::is_enabled()}).to_string()),
                            Err(e) => (500, err(&e)),
                        },
                        None => (400, err("informe iniciar_com_sistema: true/false")),
                    }
                }
                // ---- conexão com o Claude (aba "Conectar")
                (Method::Get, "/api/integracoes") => {
                    let mut v = crate::integrations::status();
                    v["ok"] = serde_json::json!(true);
                    (200, v.to_string())
                }
                (Method::Post, p) if authed && p.starts_with("/api/integracoes/") => {
                    use crate::integrations as i;
                    let action: Option<fn() -> Result<String, String>> =
                        match p.trim_start_matches("/api/integracoes/") {
                            "claude-code/conectar" => Some(i::claude_code_connect),
                            "claude-code/remover" => Some(i::claude_code_disconnect),
                            "claude-desktop/conectar" => Some(i::desktop_connect),
                            "claude-desktop/remover" => Some(i::desktop_disconnect),
                            "hooks/instalar" => Some(i::hooks_install),
                            "hooks/remover" => Some(i::hooks_remove),
                            "statusline/instalar" => Some(i::statusline_install),
                            "statusline/remover" => Some(i::statusline_remove),
                            _ => None,
                        };
                    match action.map(|f| f()) {
                        Some(Ok(msg)) => (200, serde_json::json!({"ok": true, "mensagem": msg}).to_string()),
                        Some(Err(e)) => (500, err(&e)),
                        None => (404, err("ação desconhecida")),
                    }
                }
                (Method::Post, p) if p.starts_with("/api/") => (403, err("token inválido")),
                _ => (404, err("rota desconhecida")),
            };
            if status == 200 && req.method() == &Method::Post {
                wake();
            }
            let _ = req.respond(Response::from_string(reply).with_status_code(status).with_header(content_type));
        }
    });
}
