//! Conexão do painel com o Claude: MCP (Claude Code e Claude Desktop) e hooks do Claude Code.
//! Usado pela aba "Conectar" da tela de configurações.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Map, Value, json};

const MCP_NAME: &str = "painel-led";
const HOOK_URL: &str = "127.0.0.1:7878/claude-hook";
const HOOK_CMD: &str =
    "curl -s --connect-timeout 0.3 -m 2 --data-binary @- http://127.0.0.1:7878/claude-hook >/dev/null 2>&1 || true";
/// Eventos que alimentam a lista de sessões e os avisos.
const HOOK_EVENTS: [&str; 7] =
    ["SessionStart", "UserPromptSubmit", "PostToolUse", "Notification", "Stop", "StopFailure", "SessionEnd"];
const STATUS_URL: &str = "127.0.0.1:7878/claude-statusline";
const STATUS_CMD: &str =
    "curl -s --connect-timeout 0.3 -m 1 --data-binary @- http://127.0.0.1:7878/claude-statusline 2>/dev/null || true";

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// O servidor MCP fica ao lado do executável do painel.
pub fn mcp_exe() -> PathBuf {
    let name = if cfg!(windows) { "painel-led-mcp.exe" } else { "painel-led-mcp" };
    std::env::current_exe().map(|p| p.with_file_name(name)).unwrap_or_else(|_| PathBuf::from(name))
}

fn claude_code_config() -> PathBuf {
    home().join(".claude.json")
}

fn claude_settings() -> PathBuf {
    home().join(".claude").join("settings.json")
}

fn desktop_dir() -> Option<PathBuf> {
    // Windows: %APPDATA%\Claude · macOS: ~/Library/Application Support/Claude · Linux: ~/.config/Claude
    Some(dirs::config_dir()?.join("Claude"))
}

fn desktop_config() -> Option<PathBuf> {
    Some(desktop_dir()?.join("claude_desktop_config.json"))
}

fn read_json(path: &PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Grava JSON com backup do arquivo anterior (uma única cópia ".bak-painel-led").
fn write_json(path: &PathBuf, v: &Value) -> Result<(), String> {
    if path.exists() {
        let mut bak = path.clone().into_os_string();
        bak.push(".bak-painel-led");
        std::fs::copy(path, PathBuf::from(bak)).map_err(|e| format!("backup falhou: {e}"))?;
    } else if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Procura o executável `claude` no PATH e no local padrão do instalador.
fn claude_cli() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    let in_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file())
    });
    in_path.or_else(|| {
        [home().join(".local").join("bin").join(exe), home().join(".claude").join("local").join(exe)]
            .into_iter()
            .find(|p| p.is_file())
    })
}

fn run_cli(args: &[&str]) -> Result<String, String> {
    let cli = claude_cli().ok_or("a CLI 'claude' não foi encontrada")?;
    let mut cmd = Command::new(cli);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // sem janela de console
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text.trim().to_string()) } else { Err(text.trim().to_string()) }
}

fn has_mcp(v: &Option<Value>) -> bool {
    v.as_ref().and_then(|v| v.get("mcpServers")).and_then(|m| m.get(MCP_NAME)).is_some()
}

/// (algum hook nosso instalado, todos os eventos presentes)
fn hooks_state(settings: &Option<Value>) -> (bool, bool) {
    let has = |ev: &str| {
        settings
            .as_ref()
            .and_then(|v| v.get("hooks"))
            .and_then(|h| h.get(ev))
            .and_then(|l| l.as_array())
            .is_some_and(|l| l.iter().any(is_our_group))
    };
    let n = HOOK_EVENTS.iter().filter(|e| has(e)).count();
    (n > 0, n == HOOK_EVENTS.len())
}

fn statusline_cmd(settings: &Option<Value>) -> Option<String> {
    settings.as_ref()?.get("statusLine")?.get("command")?.as_str().map(str::to_string)
}

/// Conta logada no Claude Code (campo `oauthAccount` de ~/.claude.json). Só lê dados de
/// identificação e plano; nenhum token fica nesse arquivo.
fn claude_account() -> Value {
    let Some(acc) = read_json(&claude_code_config()).and_then(|v| v.get("oauthAccount").cloned()) else {
        return Value::Null;
    };
    let get = |k: &str| acc.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
    let tier = get("userRateLimitTier").or_else(|| get("organizationRateLimitTier")).unwrap_or_default();
    let plano = match get("organizationType").as_deref() {
        Some("claude_max") if tier.contains("20x") => "Max 20x".to_string(),
        Some("claude_max") if tier.contains("5x") => "Max 5x".to_string(),
        Some("claude_max") => "Max".to_string(),
        Some("claude_pro") => "Pro".to_string(),
        Some("claude_team") => "Team".to_string(),
        Some("claude_enterprise") => "Enterprise".to_string(),
        Some(other) => other.trim_start_matches("claude_").to_string(),
        None => String::new(),
    };
    json!({
        "email": get("emailAddress"),
        "nome": get("displayName").or_else(|| get("fullName")),
        "organizacao": get("organizationName"),
        "plano": (!plano.is_empty()).then_some(plano),
    })
}

pub fn status() -> Value {
    let mcp = mcp_exe();
    let desktop = desktop_config();
    let settings = read_json(&claude_settings());
    let (hooks_any, hooks_all) = hooks_state(&settings);
    let status_cmd = statusline_cmd(&settings);
    json!({
        "mcp_exe": mcp.display().to_string(),
        "mcp_exe_existe": mcp.is_file(),
        "claude_code": {
            "conta": claude_account(),
            "cli": claude_cli().map(|p| p.display().to_string()),
            "conectado": has_mcp(&read_json(&claude_code_config())),
        },
        "claude_desktop": {
            "instalado": desktop_dir().is_some_and(|d| d.is_dir()),
            "conectado": has_mcp(&desktop.as_ref().and_then(read_json)),
            "arquivo": desktop.map(|p| p.display().to_string()),
        },
        "hooks": {
            "instalados": hooks_any,
            "completos": hooks_all,
            "eventos": HOOK_EVENTS,
            "arquivo": claude_settings().display().to_string(),
            "comando": HOOK_CMD,
        },
        "statusline": {
            "instalada": status_cmd.as_deref().is_some_and(|c| c.contains(STATUS_URL)),
            "outra": status_cmd.as_deref().filter(|c| !c.contains(STATUS_URL)),
            "comando": STATUS_CMD,
        },
    })
}

// ---------------------------------------------------------------- Claude Code (MCP)

pub fn claude_code_connect() -> Result<String, String> {
    let exe = mcp_exe();
    if !exe.is_file() {
        return Err(format!("não encontrei {}", exe.display()));
    }
    let _ = run_cli(&["mcp", "remove", MCP_NAME, "-s", "user"]);
    run_cli(&["mcp", "add", MCP_NAME, "-s", "user", "--", &exe.display().to_string()])
}

pub fn claude_code_disconnect() -> Result<String, String> {
    run_cli(&["mcp", "remove", MCP_NAME, "-s", "user"])
}

// ---------------------------------------------------------------- Claude Desktop (MCP)

pub fn desktop_connect() -> Result<String, String> {
    let path = desktop_config().ok_or("pasta de configuração não encontrada")?;
    if !path.parent().is_some_and(|d| d.is_dir()) {
        return Err("o Claude Desktop não parece estar instalado".into());
    }
    let exe = mcp_exe();
    if !exe.is_file() {
        return Err(format!("não encontrei {}", exe.display()));
    }
    let mut v = if path.exists() {
        read_json(&path).ok_or("o arquivo do Claude Desktop não é um JSON válido")?
    } else {
        json!({})
    };
    let root = v.as_object_mut().ok_or("formato inesperado")?;
    let servers = root.entry("mcpServers").or_insert_with(|| Value::Object(Map::new()));
    let servers = servers.as_object_mut().ok_or("mcpServers com formato inesperado")?;
    servers.insert(MCP_NAME.into(), json!({ "command": exe.display().to_string(), "args": [] }));
    write_json(&path, &v)?;
    Ok("Conectado. Reinicie o Claude Desktop para carregar.".into())
}

pub fn desktop_disconnect() -> Result<String, String> {
    let path = desktop_config().ok_or("pasta de configuração não encontrada")?;
    let Some(mut v) = read_json(&path) else { return Ok("Nada a remover.".into()) };
    let removed = v
        .get_mut("mcpServers")
        .and_then(|m| m.as_object_mut())
        .and_then(|m| m.shift_remove(MCP_NAME))
        .is_some();
    if removed {
        write_json(&path, &v)?;
    }
    Ok("Removido. Reinicie o Claude Desktop.".into())
}

// ---------------------------------------------------------------- hooks do Claude Code

fn is_our_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hs| hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()).is_some_and(|c| c.contains(HOOK_URL))))
}

pub fn hooks_install() -> Result<String, String> {
    let path = claude_settings();
    let mut v = if path.exists() {
        read_json(&path).ok_or("settings.json do Claude não é um JSON válido")?
    } else {
        json!({})
    };
    let root = v.as_object_mut().ok_or("formato inesperado")?;
    let hooks = root.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks.as_object_mut().ok_or("'hooks' com formato inesperado")?;
    for ev in HOOK_EVENTS {
        let list = hooks.entry(ev).or_insert_with(|| Value::Array(vec![]));
        let list = list.as_array_mut().ok_or("formato inesperado nos hooks")?;
        if !list.iter().any(is_our_group) {
            list.push(json!({ "hooks": [{ "type": "command", "command": HOOK_CMD, "timeout": 5 }] }));
        }
    }
    write_json(&path, &v)?;
    Ok("Hooks instalados. Valem para as próximas sessões do Claude Code.".into())
}

pub fn hooks_remove() -> Result<String, String> {
    let path = claude_settings();
    let Some(mut v) = read_json(&path) else { return Ok("Nada a remover.".into()) };
    if let Some(hooks) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for ev in HOOK_EVENTS {
            if let Some(list) = hooks.get_mut(ev).and_then(|l| l.as_array_mut()) {
                list.retain(|g| !is_our_group(g));
                if list.is_empty() {
                    hooks.shift_remove(ev);
                }
            }
        }
        if hooks.is_empty() {
            v.as_object_mut().unwrap().shift_remove("hooks");
        }
    }
    write_json(&path, &v)?;
    Ok("Hooks removidos.".into())
}

// ---------------------------------------------------------------- statusline do Claude Code

pub fn statusline_install() -> Result<String, String> {
    let path = claude_settings();
    let mut v = if path.exists() {
        read_json(&path).ok_or("settings.json do Claude não é um JSON válido")?
    } else {
        json!({})
    };
    if let Some(other) = statusline_cmd(&Some(v.clone())).filter(|c| !c.contains(STATUS_URL)) {
        return Err(format!(
            "você já tem uma statusline própria ({other}). Para não apagá-la, use a integração manual descrita nesta tela."
        ));
    }
    let root = v.as_object_mut().ok_or("formato inesperado")?;
    root.insert("statusLine".into(), json!({ "type": "command", "command": STATUS_CMD, "padding": 0 }));
    write_json(&path, &v)?;
    Ok("Statusline instalada. Os limites aparecem assim que o Claude Code responder a próxima mensagem.".into())
}

pub fn statusline_remove() -> Result<String, String> {
    let path = claude_settings();
    let Some(mut v) = read_json(&path) else { return Ok("Nada a remover.".into()) };
    if statusline_cmd(&Some(v.clone())).is_some_and(|c| c.contains(STATUS_URL)) {
        v.as_object_mut().ok_or("formato inesperado")?.shift_remove("statusLine");
        write_json(&path, &v)?;
    }
    Ok("Statusline removida.".into())
}
