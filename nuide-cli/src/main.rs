//! Nuide-CLI — Router de proveedores AI (gratis primero) + agente de código.
//!
//! Lógica de decisión en SpASM (AOT → C → FFI): `spasm/router.spasm` elige
//! proveedor/modelo por tarea+costo y rota cuando se agota la cuota.
//! La interfaz es una TUI (ratatui + crossterm) o un modo one-shot (`--ask`).

mod agent;
mod client;
mod config;
mod ffi;
mod tools;
mod tui;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde_json::json;

use crate::agent::{Agent, AgentEvent};
use crate::config::{Config, Source};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("nuide-cli 0.2.0 (SpASM AOT + ratatui)");
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h")
        || args.first().map(String::as_str) == Some("help")
    {
        print_help();
        return Ok(());
    }

    // Flags comunes.
    let flag = |name: &str| -> Option<String> { value_of(&args, name) };
    let free_only = args.iter().any(|a| a == "--free");
    let task = flag("--task").unwrap_or_else(|| "complete".to_string());
    let cwd = flag("--cwd")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Subcomandos.
    match args.first().map(String::as_str) {
        Some("provider") | Some("providers") => return subcommand_providers(&args),
        Some("discover") => return subcommand_discover(&args),
        Some("route") => {
            let t = args.get(1).cloned().unwrap_or(task);
            return print_route(&Config::load(), &t, free_only);
        }
        Some("ask") => {
            let prompt = args[1..].join(" ");
            if prompt.trim().is_empty() {
                bail!("uso: nuide-cli ask \"<pregunta>\"");
            }
            return run_ask(Config::load(), cwd, task, free_only, prompt);
        }
        _ => {}
    }

    // Flags de nivel superior.
    if args.iter().any(|a| a == "--providers") || args.iter().any(|a| a == "--list") {
        return list_providers(&Config::load(), free_only);
    }
    if let Some(t) = flag("--route") {
        return print_route(&Config::load(), &t, free_only);
    }
    if let Some(prompt) = flag("--ask").or_else(|| flag("-p")) {
        return run_ask(Config::load(), cwd, task, free_only, prompt);
    }

    // Por defecto: TUI.
    let cfg = Config::load();
    if cfg.candidates(free_only || cfg.free_only).is_empty() {
        eprintln!("No hay proveedores configurados.");
        eprintln!("Cargá una API key gratuita, por ejemplo:");
        eprintln!(
            "  nuide-cli provider add --id openrouter --name OpenRouter \\\n    --base-url https://openrouter.ai/api/v1 --api-key sk-or-... --free-only"
        );
        eprintln!("Config propia: {}", config::own_config_path().display());
    }
    let app = tui::App::new(Arc::new(cfg), cwd, task, free_only);
    tui::run(app)
}

// ---------------------------------------------------------------------------
// Subcomandos
// ---------------------------------------------------------------------------

fn subcommand_providers(args: &[String]) -> Result<()> {
    match args.get(1).map(String::as_str) {
        Some("add") => provider_add(args),
        Some("remove") | Some("rm") => {
            let Some(id) = args.get(2) else {
                bail!("uso: nuide-cli provider remove <id>");
            };
            let mut cfg = Config::load();
            if cfg.remove_own(id) {
                let path = cfg.save_own()?;
                println!("proveedor `{id}` eliminado ({})", path.display());
            } else {
                println!("no existe un proveedor propio con id `{id}`");
            }
            Ok(())
        }
        _ => list_providers(&Config::load(), false),
    }
}

fn provider_add(args: &[String]) -> Result<()> {
    let id = value_of(args, "--id").or_else(|| value_of(args, "--provider"));
    let base_url = value_of(args, "--base-url").or_else(|| value_of(args, "--baseURL"));
    let api_key = value_of(args, "--api-key").or_else(|| value_of(args, "--apiKey"));
    let (Some(id), Some(base_url)) = (id, base_url) else {
        bail!("uso: nuide-cli provider add --id <id> --base-url <url> [--api-key <k>] [--name <n>] [--free-only] [--model <id>]...");
    };
    let name = value_of(args, "--name").unwrap_or_else(|| id.clone());
    let free_only = args.iter().any(|a| a == "--free-only");

    let mut models = Vec::new();
    for m in values_of(args, "--model") {
        models.push(json!({ "id": m }));
    }

    let value = json!({
        "id": id,
        "name": name,
        "base_url": base_url,
        "api_key": api_key.unwrap_or_default(),
        "free_only": free_only,
        "models": models,
    });
    let Some(prov) = config::own_provider_from_value(&value) else {
        bail!("no se pudo construir el proveedor (revisá los campos)");
    };

    let mut cfg = Config::load();
    let id = prov.id.clone();
    let n = prov.models.len();
    cfg.upsert_own(prov);
    let path = cfg.save_own()?;
    println!(
        "proveedor `{id}` guardado ({n} modelo(s)) en {}",
        path.display()
    );
    Ok(())
}

fn subcommand_discover(args: &[String]) -> Result<()> {
    let Some(id) = args.get(1) else {
        bail!("uso: nuide-cli discover <provider-id> [--save-free]");
    };
    let save_free = args.iter().any(|a| a == "--save-free");
    let mut cfg = Config::load();
    let Some(prov) = cfg.provider(id).cloned() else {
        bail!("no existe el proveedor `{id}`");
    };
    let models =
        client::list_models(&prov.base_url, &prov.api_key).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{} modelo(s) en `{id}`:", models.len());
    let free: Vec<&client::ModelInfo> = models
        .iter()
        .filter(|m| m.id.contains(":free") || m.id.to_lowercase().contains("free"))
        .collect();
    for m in &models {
        let tag = if m.id.contains(":free") {
            " [free]"
        } else {
            ""
        };
        println!("  - {}{tag}", m.id);
    }
    if save_free {
        if free.is_empty() {
            println!("no se detectaron modelos gratuitos");
            return Ok(());
        }
        let all: Vec<serde_json::Value> = models
            .iter()
            .map(|m| json!({ "id": m.id, "name": m.name }))
            .collect();
        let mut value = json!({
            "id": prov.id,
            "name": prov.name,
            "base_url": prov.base_url,
            "api_key": prov.api_key,
            "free_only": true,
            "models": all,
        });
        value["free_only"] = json!(true);
        let Some(newp) = config::own_provider_from_value(&value) else {
            bail!("no se pudo reconstruir el proveedor");
        };
        cfg.upsert_own(newp);
        let path = cfg.save_own()?;
        println!(
            "guardado con {} modelo(s) (filtrando gratis) en {}",
            free.len(),
            path.display()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

fn list_providers(cfg: &Config, free_only: bool) -> Result<()> {
    let cands = cfg.candidates(free_only);
    println!(
        "Proveedores: {}  ·  candidatos: {}",
        cfg.providers.len(),
        cands.len()
    );
    for p in &cfg.providers {
        let origen = match p.source {
            Source::Own => "propia",
            Source::OpenCode => "opencode",
        };
        let key = if p.api_key.is_empty() {
            "sin key"
        } else {
            "key✓"
        };
        let fo = if p.free_only { " · free-only" } else { "" };
        println!(
            "\n• {} [{}] ({}) {}{} — {}",
            p.name, p.id, origen, key, fo, p.base_url
        );
        for m in &p.models {
            let tag = if m.free { "free" } else { "pago" };
            println!(
                "    - {}  [{} p{} c{}]",
                m.name, tag, m.power, m.cost_scaled
            );
        }
    }
    if let Some(dm) = &cfg.default_model {
        println!("\nmodelo por defecto: {dm}");
    }
    println!("\nconfig propia: {}", config::own_config_path().display());
    Ok(())
}

fn print_route(cfg: &Config, task: &str, free_only: bool) -> Result<()> {
    let clase = if ffi::router_is_power_task(task) {
        "potencia"
    } else {
        "rápida"
    };
    match cfg.route(task, &[], free_only || cfg.free_only) {
        Some(c) => {
            let tag = if c.free { "FREE" } else { "pago" };
            println!("tarea: {task} ({clase})");
            println!(
                "→ {}  [{tag}  power {}  costo {}]",
                c.label, c.power, c.cost_scaled
            );
        }
        None => println!("sin candidatos para `{task}` (configurá proveedores)"),
    }
    Ok(())
}

fn run_ask(cfg: Config, cwd: PathBuf, task: String, free_only: bool, prompt: String) -> Result<()> {
    use std::io::Write;
    let (tx, rx) = channel::<AgentEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let agent = Agent {
        config: Arc::new(cfg),
        cwd: cwd.clone(),
        task,
        free_only,
        cancel,
    };
    let conv = agent::new_conversation(&agent::system_prompt(&cwd), &prompt);
    let handle = std::thread::spawn(move || agent.run(conv, tx));

    while let Ok(ev) = rx.recv() {
        match ev {
            AgentEvent::Delta(t) => {
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
            AgentEvent::Routed { label, free } => {
                let tag = if free { "FREE" } else { "pago" };
                eprintln!("\x1b[35m· modelo [{tag}]: {label}\x1b[0m");
            }
            AgentEvent::ToolStart { name, args } => {
                let a: String = args.chars().take(140).collect();
                eprintln!("\x1b[36m🔧 {name} {a}\x1b[0m");
            }
            AgentEvent::ToolResult { name, text } => {
                let t: String = text.replace('\n', " ").chars().take(160).collect();
                eprintln!("\x1b[90m   ↳ {name}: {t}\x1b[0m");
            }
            AgentEvent::Notice(m) => eprintln!("\x1b[33m· {m}\x1b[0m"),
            AgentEvent::Error(m) => eprintln!("\x1b[31m✗ {m}\x1b[0m"),
            AgentEvent::Done { .. } => break,
        }
    }
    let _ = handle.join();
    println!();
    Ok(())
}

/// Valor de `--flag valor` o `--flag=valor`.
fn value_of(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

/// Todos los valores repetidos de `--flag valor`.
fn values_of(args: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            if let Some(v) = it.next() {
                out.push(v.clone());
            }
        } else if let Some(rest) = a.strip_prefix(&format!("{flag}=")) {
            out.push(rest.to_string());
        }
    }
    out
}

fn print_help() {
    println!("nuide-cli 0.2.0 — Router de modelos AI (gratis primero) + coder");
    println!();
    println!("Uso:");
    println!("  nuide-cli                          Abre la TUI (chat + herramientas)");
    println!("  nuide-cli ask \"<pregunta>\"         Modo one-shot (imprime la respuesta)");
    println!("  nuide-cli --route <task>           Muestra qué proveedor/modelo elegiría");
    println!("  nuide-cli --providers              Lista proveedores y modelos");
    println!("  nuide-cli provider add ...         Alta de proveedor (con su API key)");
    println!("  nuide-cli provider remove <id>     Baja de proveedor propio");
    println!("  nuide-cli discover <id> [--save-free]  Lista modelos del endpoint");
    println!("  nuide-cli --version | --help");
    println!();
    println!("Flags:");
    println!("  --free            Sólo modelos gratuitos");
    println!("  --task <t>        Tarea (complete|refactor|debug|explain|test|architect)");
    println!("  --cwd <path>      Raíz del proyecto (default: directorio actual)");
    println!();
    println!("Ejemplo (OpenRouter gratis):");
    println!("  nuide-cli provider add --id openrouter --name OpenRouter \\");
    println!("      --base-url https://openrouter.ai/api/v1 --api-key sk-or-... --free-only");
    println!();
    println!("Config propia: {}", config::own_config_path().display());
}
