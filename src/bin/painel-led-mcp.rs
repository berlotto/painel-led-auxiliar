//! Servidor MCP (stdio) do Painel LED.
//! Repassa as ferramentas para a API local do painel (127.0.0.1:7878) e abre o painel se ele estiver fechado.

use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BASE: &str = "http://127.0.0.1:7878";

const CORES: [&str; 9] = ["ambar", "verde", "vermelho", "ciano", "branco", "laranja", "rosa", "azul", "amarelo"];

fn tools() -> Value {
    let cor = json!({
        "type": "string",
        "description": format!("Cor: {} ou \"#rrggbb\".", CORES.join(", "))
    });
    json!([
        {
            "name": "mostrar_mensagem",
            "description": "Mostra uma mensagem no painel de LED do usuário (letreiro sempre visível na tela dele). \
                Use para avisos curtos e úteis: 'build terminou', 'testes passaram', resumos de uma linha. \
                Textos curtos (até ~60 caracteres) funcionam melhor.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "texto": { "type": "string", "description": "Texto a exibir." },
                    "cor": cor,
                    "efeito": {
                        "type": "string", "enum": ["rolar", "fixo", "piscar"],
                        "description": "rolar (padrão): passa pelo painel; fixo: fica parado alguns segundos; piscar: alerta que fura a fila."
                    },
                    "repetir": { "type": "integer", "minimum": 1, "maximum": 20, "description": "Quantas vezes o texto rola (padrão 2)." },
                    "segundos": { "type": "number", "minimum": 1, "maximum": 3600, "description": "Tempo na tela para fixo/piscar (padrão 8)." }
                },
                "required": ["texto"]
            }
        },
        {
            "name": "alerta",
            "description": "Alerta importante no painel: pisca em vermelho (ou na cor indicada) e passa na frente das outras mensagens. \
                Reserve para coisas que exigem atenção imediata do usuário.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "texto": { "type": "string" },
                    "cor": cor,
                    "segundos": { "type": "number", "minimum": 1, "maximum": 3600, "description": "Tempo piscando (padrão 10)." }
                },
                "required": ["texto"]
            }
        },
        {
            "name": "lembrete",
            "description": "Agenda uma mensagem para aparecer piscando no painel daqui a N minutos (ex.: 'me lembra em 20 min de ligar pro João'). \
                O lembrete fica no painel; se o painel for fechado antes, ele se perde.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "texto": { "type": "string" },
                    "minutos": { "type": "number", "minimum": 0.1, "maximum": 1440 },
                    "cor": cor
                },
                "required": ["texto", "minutos"]
            }
        },
        {
            "name": "definir_texto",
            "description": "Define um texto nomeado que fica fixo no painel enquanto existir (aparece nos blocos configurados com o item 'var:NOME', \
                por exemplo var:status). Útil para status contínuo: 'Claude: refatorando auth'. Texto vazio apaga.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "nome": { "type": "string", "description": "Nome do texto, ex.: status." },
                    "texto": { "type": "string", "description": "Conteúdo; vazio apaga." },
                    "cor": cor
                },
                "required": ["nome", "texto"]
            }
        },
        {
            "name": "limpar_mensagens",
            "description": "Descarta as mensagens e lembretes pendentes no painel.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "sessoes_claude",
            "description": "Lista as sessões do Claude Code abertas neste computador (projeto, estado: trabalhando,                 precisa_de_voce, esperando, concluida, aberta; modelo, contexto, custo) e os limites de uso do plano                 (% usado nas janelas de 5 horas e 7 dias e quando renovam). Use quando o usuário perguntar                 'alguma sessão precisa de mim?', 'quanto do limite já usei?' ou antes de iniciar uma tarefa longa.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "status_painel",
            "description": "Lê o estado do painel: CPU, RAM, conexão com a internet, próximos compromissos da agenda, textos nomeados e fila de mensagens.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(3)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn request(method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    let url = format!("{BASE}{path}");
    let a = agent();
    let resp = match (method, body) {
        ("GET", _) => a.get(&url).call(),
        (_, Some(b)) => a.post(&url).header("Content-Type", "application/json").send(b.to_string()),
        (_, None) => a.post(&url).send(""),
    };
    let mut resp = resp.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    if status >= 400 {
        return Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("erro").to_string());
    }
    Ok(v)
}

/// Se o painel não responde, tenta abri-lo (o executável fica na mesma pasta) e espera até 5 s.
fn ensure_panel() -> Result<(), String> {
    if request("GET", "/status", None).is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = if cfg!(windows) { "painel-led.exe" } else { "painel-led" };
    let panel = exe.with_file_name(name);
    std::process::Command::new(&panel)
        .spawn()
        .map_err(|e| format!("painel fechado e não consegui abrir {}: {e}", panel.display()))?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(200));
        if request("GET", "/status", None).is_ok() {
            return Ok(());
        }
    }
    Err("o painel não respondeu depois de aberto".into())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn call(params: &Value) -> Result<String, String> {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let texto = str_arg(&args, "texto").unwrap_or("").trim().to_string();
    let mut body = json!({ "texto": texto });
    if let Some(c) = str_arg(&args, "cor") {
        body["cor"] = json!(c);
    }

    ensure_panel()?;
    match name {
        "mostrar_mensagem" => {
            for k in ["efeito", "repetir", "segundos"] {
                if let Some(v) = args.get(k) {
                    body[k] = v.clone();
                }
            }
            request("POST", "/mensagem", Some(&body))?;
            Ok("Mensagem exibida no painel.".into())
        }
        "alerta" => {
            body["efeito"] = json!("piscar");
            if let Some(v) = args.get("segundos") {
                body["segundos"] = v.clone();
            }
            request("POST", "/mensagem", Some(&body))?;
            Ok("Alerta piscando no painel.".into())
        }
        "lembrete" => {
            let min = args.get("minutos").and_then(|m| m.as_f64()).ok_or("informe 'minutos'")?;
            body["efeito"] = json!("piscar");
            body["em_minutos"] = json!(min);
            if body.get("cor").is_none() {
                body["cor"] = json!("laranja");
            }
            body["segundos"] = json!(20);
            let r = request("POST", "/mensagem", Some(&body))?;
            let when = r.get("agendado_para").and_then(|v| v.as_str()).unwrap_or("?");
            Ok(format!("Lembrete agendado para {when}."))
        }
        "definir_texto" => {
            let nome = str_arg(&args, "nome").ok_or("informe 'nome'")?;
            body["nome"] = json!(nome);
            request("POST", "/texto", Some(&body))?;
            Ok(if texto.is_empty() {
                format!("Texto '{nome}' apagado.")
            } else {
                format!("Texto '{nome}' definido. Ele aparece nos blocos com o item var:{nome}.")
            })
        }
        "limpar_mensagens" => {
            request("POST", "/limpar", None)?;
            Ok("Mensagens e lembretes descartados.".into())
        }
        "sessoes_claude" => {
            let v = request("GET", "/claude", None)?;
            Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
        }
        "status_painel" => {
            let v = request("GET", "/status", None)?;
            Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
        }
        other => Err(format!("ferramenta desconhecida: {other}")),
    }
}

fn main() {
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                let e = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "JSON inválido"}});
                let _ = writeln!(out, "{e}");
                let _ = out.flush();
                continue;
            }
        };
        let Some(id) = req.get("id").cloned() else { continue }; // notificação: sem resposta
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));

        let result: Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "painel-led", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Painel de LED sempre visível na tela do usuário. Use para avisos curtos, alertas, \
                    lembretes e status contínuo. Não envie textos longos nem mensagens em excesso."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let (text, is_error) = match call(&params) {
                    Ok(t) => (t, false),
                    Err(e) => (format!("Erro: {e}"), true),
                };
                Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
            }
            _ => Err((-32601, format!("método não suportado: {method}"))),
        };
        let resp = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        };
        let _ = writeln!(out, "{resp}");
        let _ = out.flush();
    }
}
