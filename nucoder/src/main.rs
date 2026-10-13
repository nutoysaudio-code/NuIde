//! NuCoder — Router de proveedores AI (gratis primero) + agente de código.
//!
//! Lógica de decisión en SpASM (AOT → C → FFI): `spasm/router.spasm` elige
//! proveedor/modelo por tarea+costo y rota cuando se agota la cuota.
//!
//! Este binario es el **CLI**: subcomandos (`ask`, `provider`, `models`,
//! `discover`, `route`) y `--version/--help`. La interfaz de chat interactiva
//! vive en **NuIde** como panel nativo (Slint); el núcleo está en la librería
//! `nucoder` (`src/lib.rs`).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde_json::json;

use nucoder::agent::{self, Agent, AgentEvent};
use nucoder::caps::{self, Cap};
use nucoder::client;
use nucoder::config::{self, Config};
use nucoder::cook;
use nucoder::freefilter::{self, FreeFilter};
use nucoder::image;
use nucoder::knowledge::Knowledge;
use nucoder::mcp::{McpClient, CALL_TIMEOUT};
use nucoder::skills::{Registry, SkillEffect};
use nucoder::{banner, ffi};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "nucoder {} (SpASM AOT + núcleo compartido)",
            env!("CARGO_PKG_VERSION")
        );
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
    let cap = flag("--cap").and_then(|s| Cap::parse(&s));
    let cli_model = flag("--model").filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("auto"));
    let task = flag("--task").unwrap_or_else(|| "complete".to_string());
    let cwd = flag("--cwd")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Subcomandos.
    match args.first().map(String::as_str) {
        Some("provider") | Some("providers") => return subcommand_providers(&args),
        Some("discover") => return subcommand_discover(&args),
        Some("refresh") => return subcommand_refresh(&args),
        Some("models") => return subcommand_models(&args),
        Some("config") | Some("cfg") => return subcommand_config(&args),
        Some("skills") => return subcommand_skills(&args),
        Some("skill") => return subcommand_skill(&args),
        Some("cook") => return subcommand_cook(&args),
        Some("mcp") => return subcommand_mcp(&args),
        Some("route") => {
            let t = args.get(1).cloned().unwrap_or(task);
            let cfg = load_config();
            return print_route(&cfg, &t, free_only, cap);
        }
        Some("ask") => {
            let prompt = ask_prompt(&args);
            if prompt.trim().is_empty() {
                bail!("uso: nucoder ask \"<pregunta>\"");
            }
            let cfg = load_config();
            let sandbox = resolve_sandbox(&cfg, &args);
            let images = load_images(&args)?;
            let model_override = cli_model.clone().or_else(|| cfg.default_model.clone());
            let prompt_cache = resolve_prompt_cache(&cfg, &args);
            let context_chars = resolve_context(&cfg, &args);
            let skill_preload = flag("--skill");
            return run_ask(
                cfg,
                cwd,
                task,
                free_only,
                prompt,
                sandbox,
                cap,
                images,
                model_override,
                prompt_cache,
                context_chars,
                skill_preload,
            );
        }
        _ => {}
    }

    // Flags de nivel superior.
    if args.iter().any(|a| a == "--providers") || args.iter().any(|a| a == "--list") {
        let cfg = load_config();
        return list_providers(&cfg, free_only);
    }
    if let Some(t) = flag("--route") {
        let cfg = load_config();
        return print_route(&cfg, &t, free_only, cap);
    }
    if let Some(prompt) = flag("--ask").or_else(|| flag("-p")) {
        let cfg = load_config();
        let sandbox = resolve_sandbox(&cfg, &args);
        let images = load_images(&args)?;
        let model_override = cli_model.clone().or_else(|| cfg.default_model.clone());
        let prompt_cache = resolve_prompt_cache(&cfg, &args);
        let context_chars = resolve_context(&cfg, &args);
        let skill_preload = flag("--skill");
        return run_ask(
            cfg,
            cwd,
            task,
            free_only,
            prompt,
            sandbox,
            cap,
            images,
            model_override,
            prompt_cache,
            context_chars,
            skill_preload,
        );
    }

    // Sin subcomando: el chat interactivo vive en NuIde (panel nativo Slint).
    // El CLI queda para `ask`, `provider`, `models`, `discover` y `route`.
    print_help();
    println!("La interfaz de chat de NuCoder ahora vive en NuIde (panel «Agente»).");
    println!("Para una respuesta suelta usá:  nucoder ask \"<pregunta>\"");
    Ok(())
}

/// Sandbox efectivo: `--no-sandbox` / `--sandbox` pisan la config.
fn resolve_sandbox(cfg: &Config, args: &[String]) -> bool {
    if args.iter().any(|a| a == "--no-sandbox") {
        return false;
    }
    if args.iter().any(|a| a == "--sandbox") {
        return true;
    }
    cfg.sandbox
}

/// Prompt caching: `--prompt-cache` fuerza; si no, lo que diga la config.
fn resolve_prompt_cache(cfg: &Config, args: &[String]) -> bool {
    args.iter().any(|a| a == "--prompt-cache") || cfg.prompt_cache
}

/// Tope de contexto efectivo: `--context-chars` > `NUCODER_CONTEXT_CHARS` > config.
fn resolve_context(cfg: &Config, args: &[String]) -> usize {
    if let Some(n) = value_of(args, "--context-chars").and_then(|s| s.parse::<usize>().ok()) {
        return n.max(2_000);
    }
    if let Ok(s) = std::env::var("NUCODER_CONTEXT_CHARS") {
        if let Ok(n) = s.parse::<usize>() {
            return n.max(2_000);
        }
    }
    cfg.context_chars
}

// ---------------------------------------------------------------------------
// Subcomandos
// ---------------------------------------------------------------------------

fn subcommand_providers(args: &[String]) -> Result<()> {
    match args.get(1).map(String::as_str) {
        Some("add") => provider_add(args),
        Some("presets") | Some("preset") => print_presets(),
        Some("remove") | Some("rm") => {
            let Some(id) = args.get(2) else {
                bail!("uso: nucoder provider remove <id>");
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
        _ => {
            let cfg = load_config();
            list_providers(&cfg, false)
        }
    }
}

/// Lista los presets conocidos de proveedores.
fn print_presets() -> Result<()> {
    println!("Presets de proveedor (id → base URL · filtro de gratis):");
    for p in freefilter::all() {
        println!(
            "  {:<12} {:<52} {}",
            p.id,
            p.base_url,
            p.free_filter.as_str()
        );
    }
    println!("\nAlta:  nucoder provider add <id> --api-key <key>");
    println!("Luego: nucoder discover <id> --save-free");
    Ok(())
}

fn provider_add(args: &[String]) -> Result<()> {
    let Some(id) = value_of(args, "--id")
        .or_else(|| value_of(args, "--provider"))
        .or_else(|| args.get(2).filter(|a| !a.starts_with('-')).cloned())
    else {
        bail!(
            "uso: nucoder provider add <id> [--base-url <url>] [--api-key <k>] \
             [--name <n>] [--free-only] [--free-filter <modo>] [--model <id>]...\n\
             ids con preset: {}",
            preset_ids()
        );
    };
    let preset = freefilter::preset(&id);
    // La base URL sale del preset si no se pasa a mano.
    let base_url = value_of(args, "--base-url")
        .or_else(|| value_of(args, "--baseURL"))
        .or_else(|| preset.map(|p| p.base_url.to_string()));
    let Some(base_url) = base_url else {
        bail!("falta --base-url (y `{id}` no es un preset conocido)");
    };
    let api_key = value_of(args, "--api-key").or_else(|| value_of(args, "--apiKey"));
    let name = value_of(args, "--name")
        .or_else(|| preset.map(|p| p.name.to_string()))
        .unwrap_or_else(|| id.clone());
    // Preset ⇒ free-only salvo `--no-free-only` (son catálogos free-tier).
    let free_only = args.iter().any(|a| a == "--free-only")
        || (preset.is_some() && !args.iter().any(|a| a == "--no-free-only"));
    let free_filter = value_of(args, "--free-filter")
        .and_then(|s| FreeFilter::parse(&s))
        .or_else(|| preset.map(|p| p.free_filter))
        .unwrap_or(FreeFilter::Auto);
    let free_match = values_of(args, "--free-match");

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
        "free_filter": free_filter.as_str(),
        "free_match": free_match,
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
        "proveedor `{id}` guardado ({n} modelo(s), filtro {}) en {}",
        free_filter.as_str(),
        path.display()
    );
    Ok(())
}

/// Ids de los presets conocidos.
fn preset_ids() -> String {
    freefilter::all()
        .iter()
        .map(|p| p.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `nucoder refresh [id] [--reset-free]`: re-baja el catálogo y reclasifica los
/// modelos **conservando** los `free` fijados a mano (salvo `--reset-free`).
fn subcommand_refresh(args: &[String]) -> Result<()> {
    let only = args.get(1).filter(|a| !a.starts_with('-')).cloned();
    let reset_free = args.iter().any(|a| a == "--reset-free");
    let mut cfg = Config::load();
    if cfg.providers.is_empty() {
        bail!("no hay proveedores configurados (probá `nucoder provider add <id>`)");
    }
    let results = config::refresh_catalog(&mut cfg, only.as_deref(), reset_free);
    for (id, res) in results {
        match res {
            Ok(n) => println!(
                "· `{id}`: {n} modelo(s){}",
                if reset_free {
                    " (free recalculado)"
                } else {
                    " (free conservado)"
                }
            ),
            Err(e) => eprintln!("· `{id}`: {e}"),
        }
    }
    let path = cfg.save_own()?;
    println!("guardado en {}", path.display());
    Ok(())
}

fn subcommand_discover(args: &[String]) -> Result<()> {
    let Some(id) = args.get(1) else {
        bail!("uso: nucoder discover <provider-id> [--save-free|--save-all]");
    };
    let save_free = args.iter().any(|a| a == "--save-free");
    let save_all = args.iter().any(|a| a == "--save-all");
    let mut cfg = Config::load();
    let Some(prov) = cfg.provider(id).cloned() else {
        bail!("no existe el proveedor `{id}`");
    };
    let models =
        client::list_models(&prov.base_url, &prov.api_key).map_err(|e| anyhow::anyhow!("{e}"))?;

    let flags: Vec<bool> = models.iter().map(|m| model_free(&prov, m)).collect();
    println!(
        "{} modelo(s) en `{id}` (filtro de gratis: {}):",
        models.len(),
        prov.free_filter.as_str()
    );
    for (i, m) in models.iter().enumerate() {
        let tag = if flags[i] { " [free]" } else { "" };
        let caps = caps_label(&m.id, &m.name, &m.raw);
        println!("  - {}{tag}  ({caps})", m.id);
    }

    if !(save_free || save_all) {
        return Ok(());
    }
    let selected: Vec<usize> = (0..models.len())
        .filter(|&i| save_all || flags[i])
        .collect();
    if selected.is_empty() {
        println!(
            "no se detectaron modelos gratuitos con el filtro `{}`; \
             probá `--save-all` o ajustá `free_filter` del proveedor.",
            prov.free_filter.as_str()
        );
        return Ok(());
    }
    let list: Vec<serde_json::Value> = selected
        .iter()
        .map(|&i| discovered_model(&models[i], flags[i]))
        .collect();
    let value = provider_value(&prov, &list);
    let Some(newp) = config::own_provider_from_value(&value) else {
        bail!("no se pudo reconstruir el proveedor");
    };
    let n = list.len();
    cfg.upsert_own(newp);
    let path = cfg.save_own()?;
    println!("guardado con {n} modelo(s) en {}", path.display());
    Ok(())
}

/// ¿El modelo es gratis según el filtro del proveedor?
fn model_free(prov: &config::Provider, m: &client::ModelInfo) -> bool {
    freefilter::classify(
        prov.free_filter,
        &m.raw,
        &m.id,
        &m.name,
        &prov.free_match,
        prov.free_only,
    )
}

/// Capacidades separadas por `+`, para mostrar.
fn caps_label(id: &str, name: &str, raw: &serde_json::Value) -> String {
    caps::infer(id, name, raw)
        .iter()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join("+")
}

/// Un modelo descubierto, listo para persistir (con `free` y `caps` explícitos).
fn discovered_model(m: &client::ModelInfo, free: bool) -> serde_json::Value {
    let caps: Vec<&str> = caps::infer(&m.id, &m.name, &m.raw)
        .iter()
        .map(|c| c.as_str())
        .collect();
    json!({ "id": m.id, "name": m.name, "free": free, "caps": caps })
}

/// JSON del proveedor para `own_provider_from_value` (conserva el filtro).
fn provider_value(prov: &config::Provider, models: &[serde_json::Value]) -> serde_json::Value {
    json!({
        "id": prov.id,
        "name": prov.name,
        "base_url": prov.base_url,
        "api_key": prov.api_key,
        "free_only": prov.free_only,
        "free_filter": prov.free_filter.as_str(),
        "free_match": prov.free_match,
        "models": models,
    })
}

/// Carga la config y, si un proveedor `free_only` quedó sin modelos, los
/// descubre en su endpoint y los guarda (`discover --save-all` automático).
/// Best-effort: si falla, avisa y sigue (no bloquea el arranque).
fn load_config() -> Config {
    let mut cfg = Config::load();
    auto_discover(&mut cfg);
    cfg
}

fn auto_discover(cfg: &mut Config) {
    let missing: Vec<usize> = cfg
        .providers
        .iter()
        .enumerate()
        .filter(|(_, p)| p.free_only && p.models.is_empty() && !p.base_url.trim().is_empty())
        .map(|(i, _)| i)
        .collect();
    if missing.is_empty() {
        return;
    }
    let mut changed = false;
    for i in missing {
        let p = cfg.providers[i].clone();
        match client::list_models(&p.base_url, &p.api_key) {
            Ok(models) if !models.is_empty() => {
                let list: Vec<serde_json::Value> = models
                    .iter()
                    .map(|m| discovered_model(m, model_free(&p, m)))
                    .collect();
                let value = provider_value(&p, &list);
                if let Some(np) = config::own_provider_from_value(&value) {
                    eprintln!(
                        "· `{}`: {} modelo(s) descubiertos automáticamente.",
                        p.id,
                        list.len()
                    );
                    cfg.upsert_own(np);
                    changed = true;
                }
            }
            Ok(_) => eprintln!("· `{}`: el endpoint no devolvió modelos.", p.id),
            Err(e) => eprintln!("· `{}`: no se pudo descubrir modelos ({e}).", p.id),
        }
    }
    if changed {
        let _ = cfg.save_own();
    }
}

/// Lista/filtra modelos: `nucoder models [--free] [--cap <c>] [--provider <id>]`.
fn subcommand_models(args: &[String]) -> Result<()> {
    let free = args.iter().any(|a| a == "--free");
    let cap = value_of(args, "--cap").and_then(|s| Cap::parse(&s));
    let only_prov = value_of(args, "--provider");
    let cfg = load_config();
    let cands = cfg.candidates(free || cfg.free_only, cap, false);
    let mut rows: Vec<(String, i64, bool, String)> = Vec::new();
    for c in &cands {
        let p = &cfg.providers[c.provider_idx];
        if let Some(id) = &only_prov {
            if &p.id != id {
                continue;
            }
        }
        let m = &p.models[c.model_idx];
        let caps: Vec<&str> = m.caps.iter().map(|x| x.as_str()).collect();
        rows.push((
            caps.join("+"),
            c.power as i64,
            c.free,
            format!("{}/{}", p.id, m.id),
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    if rows.is_empty() {
        println!("sin modelos (probá `nucoder discover <id>`)");
        return Ok(());
    }
    let filtro = cap
        .map(|c| format!(" · cap {}", c.as_str()))
        .unwrap_or_default();
    println!("{} modelo(s){}:", rows.len(), filtro);
    for (caps, power, free, label) in &rows {
        let tag = if *free { "free" } else { "pago" };
        println!("  [{tag:<4}] p{power}  {caps:<20} {label}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Configuración (switch de auto-aprobar, sandbox, gratis…)
// ---------------------------------------------------------------------------

/// `nucoder config` muestra los ajustes; `nucoder config <clave> <valor>` los
/// cambia y los persiste en la config propia.
fn subcommand_config(args: &[String]) -> Result<()> {
    let mut cfg = Config::load();
    let Some(key) = args.get(1) else {
        return print_config(&cfg);
    };
    let Some(val) = args.get(2) else {
        bail!(
            "uso: nucoder config <clave> <valor>\n\
             claves: auto-approve, autopilot, internet, leer-fuera, terminales, ftp, \
             confirm, sandbox, free-only, context-chars, default-model, skills-autoinvoke"
        );
    };
    match key.as_str() {
        "auto-approve" | "auto_approve" | "auto" => cfg.auto_approve = parse_on_off(val)?,
        "confirm" => cfg.confirm = parse_on_off(val)?,
        "sandbox" => cfg.sandbox = parse_on_off(val)?,
        "free-only" | "free_only" | "free" => cfg.free_only = parse_on_off(val)?,
        "context-chars" | "context" => {
            let n = val
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("context-chars necesita un número"))?;
            cfg.context_chars = n.max(2_000);
        }
        "default-model" | "model" => {
            cfg.default_model = if val.eq_ignore_ascii_case("auto") || val == "none" {
                None
            } else {
                Some(val.clone())
            }
        }
        "web" | "internet" => cfg.perm_web = parse_on_off(val)?,
        "read-outside" | "leer-fuera" => cfg.perm_read_outside = parse_on_off(val)?,
        "terminal" | "terminales" => cfg.perm_terminal = parse_on_off(val)?,
        "ftp" | "cliente-ftp" => cfg.perm_ftp = parse_on_off(val)?,
        "autopilot" | "autopiloto" => cfg.autopilot = parse_on_off(val)?,
        "skills-autoinvoke" | "skills" => cfg.skills_autoinvoke = parse_on_off(val)?,
        other => bail!("clave desconocida: {other}"),
    }
    let path = cfg.save_own()?;
    print_config(&cfg)?;
    println!("\nguardado en {}", path.display());
    Ok(())
}

/// `nucoder skills`: lista los skills descubiertos (Claude/OpenCode/nativos).
fn subcommand_skills(_args: &[String]) -> Result<()> {
    let cfg = Config::load();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let reg = Registry::discover(
        &cwd,
        &cfg.skills,
        &cfg.skills_permissions,
        cfg.skills_autoinvoke,
    );
    if reg.is_empty() {
        println!("no se encontraron skills.");
        println!("Se buscan en: ~/.claude/skills, .claude/skills, ~/.agents/skills,");
        println!("             .agents/skills, ~/.config/opencode/skills, .opencode/skills");
        println!("             y SPASM_HOME/skills (nativos). Sumá rutas con la");
        println!("             config `skills` o la variable `NUCODER_SKILLS`.");
        return Ok(());
    }
    println!(
        "{} skill(s) ({} se anuncian al modelo):",
        reg.skills().len(),
        reg.advertised().count()
    );
    for sk in reg.skills() {
        let estado = match reg.effect(&sk.id) {
            SkillEffect::Deny => "  (deny)",
            SkillEffect::Ask => "  (ask: pide confirmación)",
            SkillEffect::Allow if !reg.advertises(sk) => "  (oculto)",
            SkillEffect::Allow => "",
        };
        println!("  {}{estado}", sk.line());
        if let Some(v) = &sk.version {
            print!("      v{v}");
        }
        if !sk.capabilities.is_empty() {
            print!("  · {}", sk.capabilities.join(", "));
        }
        if sk.version.is_some() || !sk.capabilities.is_empty() {
            println!();
        }
    }
    println!("\ncargar/ver uno:  nucoder skill <id>");
    Ok(())
}

/// `nucoder cook <skill-id|ruta> [--gourmet] [--model <id>] [--refresh]`: destila
/// una **habilidad** (NuCook) quitando el ruido humano, sin perder nada en silencio.
fn subcommand_cook(args: &[String]) -> Result<()> {
    let json = args.iter().any(|a| a == "--json");
    let stats = args.iter().any(|a| a == "--stats");
    let gourmet = args.iter().any(|a| a == "--gourmet" || a == "--semantic");
    let refresh = args.iter().any(|a| a == "--refresh");
    let model = value_of(args, "--model");
    let Some(arg) = args.get(1).filter(|a| !a.starts_with('-')) else {
        bail!(
            "uso: nucoder cook <skill-id|ruta> [--gourmet] [--model <id>] [--refresh] \
             [--json] [--stats]"
        );
    };
    let is_file = Path::new(arg).is_file();
    let teaching = if gourmet {
        let cfg = Config::load();
        if is_file {
            let p = Path::new(arg);
            let text = std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{arg}: {e}"))?;
            let body = nucoder::skills::strip_frontmatter(&text);
            let id = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".to_string());
            cook::gourmet_body(&id, p, &body, &cfg, model.as_deref(), refresh)
                .map_err(|e| anyhow::anyhow!("{e}"))?
        } else {
            let sk = find_skill(&cfg, arg)?;
            cook::cook_skill_gourmet(&sk, &cfg, model.as_deref(), refresh)
                .map_err(|e| anyhow::anyhow!("{e}"))?
        }
    } else if is_file {
        cook::cook_path(Path::new(arg)).map_err(|e| anyhow::anyhow!("{e}"))?
    } else {
        let cfg = Config::load();
        let sk = find_skill(&cfg, arg)?;
        cook::cook_skill(&sk).map_err(|e| anyhow::anyhow!("{e}"))?
    };
    if stats {
        eprintln!(
            "· {} · {} unidad(es) · {} conservada(s) · {} ruido · {} duplicado(s)",
            teaching.mode,
            teaching.stats.units,
            teaching.stats.kept,
            teaching.stats.noise,
            teaching.stats.dup
        );
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&teaching.to_json())?);
    } else {
        print!("{}", teaching.render_text());
    }
    Ok(())
}

/// Resuelve un skill por id (copia propia: el `Registry` es local al proceso).
fn find_skill(cfg: &Config, id: &str) -> Result<nucoder::skills::Skill> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let reg = Registry::discover(
        &cwd,
        &cfg.skills,
        &cfg.skills_permissions,
        cfg.skills_autoinvoke,
    );
    reg.by_id(id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no existe el skill `{id}` (ni un archivo con esa ruta)"))
}

/// `nucoder mcp list|tools <id>|call <id> <tool> [json]`: administra los
/// servidores MCP configurados.
fn subcommand_mcp(args: &[String]) -> Result<()> {
    let cfg = Config::load();
    match args.get(1).map(String::as_str) {
        None | Some("list") => {
            if cfg.mcp.is_empty() {
                println!("no hay servidores MCP configurados (config `mcp.servers`).");
                println!("Ejemplo: {{\"mcp\":{{\"servers\":{{\"rea\":{{\"command\":\"npx\",");
                println!("  \"args\":[\"-y\",\"rea-agents@6.4.0\",\"mcp\"]}}}}}}}}");
                return Ok(());
            }
            println!("{} servidor(es) MCP:", cfg.mcp.len());
            for s in &cfg.mcp {
                println!(
                    "  {:<12} {:<3} {}{}",
                    s.id,
                    if s.enabled { "on" } else { "off" },
                    s.command,
                    if s.args.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", s.args.join(" "))
                    }
                );
            }
            Ok(())
        }
        Some("tools") => {
            let Some(id) = args.get(2).filter(|a| !a.starts_with('-')) else {
                bail!("uso: nucoder mcp tools <server>");
            };
            let srv = find_mcp(&cfg, id)?;
            let mut c = McpClient::spawn(srv).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{} · {} herramienta(s):", c.server_name, c.tools.len());
            for t in &c.tools {
                let d: String = t.description.chars().take(80).collect();
                println!("  {:<34} {}", t.name, d);
            }
            c.shutdown();
            Ok(())
        }
        Some("call") => {
            let id = args.get(2).filter(|a| !a.starts_with('-'));
            let tool = args.get(3).filter(|a| !a.starts_with('-'));
            let (Some(id), Some(tool)) = (id, tool) else {
                bail!("uso: nucoder mcp call <server> <tool> [json]");
            };
            let payload = match args.get(4) {
                Some(j) => serde_json::from_str::<serde_json::Value>(j)
                    .map_err(|e| anyhow::anyhow!("json inválido: {e}"))?,
                None => serde_json::json!({}),
            };
            let srv = find_mcp(&cfg, id)?;
            let mut c = McpClient::spawn(srv).map_err(|e| anyhow::anyhow!("{e}"))?;
            let out = c
                .call(tool, &payload, CALL_TIMEOUT)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{out}");
            c.shutdown();
            Ok(())
        }
        Some(other) => bail!("subcomando mcp desconocido: {other} (usá list|tools|call)"),
    }
}

/// Busca un servidor MCP por id.
fn find_mcp<'a>(cfg: &'a Config, id: &str) -> Result<&'a nucoder::mcp::McpServer> {
    cfg.mcp
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| anyhow::anyhow!("no existe el servidor MCP `{id}`"))
}

/// `nucoder skill <id>`: imprime el cuerpo (sin frontmatter) del skill.
fn subcommand_skill(args: &[String]) -> Result<()> {
    let Some(id) = args.get(1).filter(|a| !a.starts_with('-')) else {
        bail!("uso: nucoder skill <id>   (listá con `nucoder skills`)");
    };
    let cfg = Config::load();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let reg = Registry::discover(
        &cwd,
        &cfg.skills,
        &cfg.skills_permissions,
        cfg.skills_autoinvoke,
    );
    let body = reg.load(id).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{body}");
    Ok(())
}

fn print_config(cfg: &Config) -> Result<()> {
    println!(
        "Config de NuCoder ({}):",
        config::own_config_path().display()
    );
    println!(
        "  auto-approve    {}   (no pedir confirmación para borrar/mover/bash)",
        on_off(cfg.auto_approve)
    );
    println!("  confirm         {}", on_off(cfg.confirm));
    println!("  sandbox         {}", on_off(cfg.sandbox));
    println!("  free-only       {}", on_off(cfg.free_only));
    println!("  context-chars   {}", cfg.context_chars);
    println!(
        "  default-model   {}",
        cfg.default_model.as_deref().unwrap_or("auto (router)")
    );
    println!("  permisos:");
    println!("    internet      {}", on_off(cfg.perm_web));
    println!("    leer-fuera    {}", on_off(cfg.perm_read_outside));
    println!("    terminales    {}", on_off(cfg.perm_terminal));
    println!("    ftp           {}", on_off(cfg.perm_ftp));
    println!("    autopiloto    {}", on_off(cfg.autopilot));
    println!("  skills:");
    println!("    autoinvoke    {}", on_off(cfg.skills_autoinvoke));
    for r in &cfg.skills_permissions {
        println!("    {:<5}         {}", r.effect.as_str(), r.id);
    }
    println!(
        "  efectivo: {}",
        if cfg.asks_confirmation() {
            "pide confirmación"
        } else {
            "auto-aprueba (no pregunta)"
        }
    );
    Ok(())
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on "
    } else {
        "off"
    }
}

/// `on/true/1/yes` → true; `off/false/0/no` → false.
fn parse_on_off(v: &str) -> Result<bool> {
    match v.to_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "si" | "sí" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        other => bail!("valor inválido `{other}` (usá on/off)"),
    }
}

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

fn list_providers(cfg: &Config, free_only: bool) -> Result<()> {
    let cands = cfg.candidates(free_only, None, false);
    println!(
        "Proveedores: {}  ·  candidatos: {}",
        cfg.providers.len(),
        cands.len()
    );
    for p in &cfg.providers {
        let key = if p.api_key.is_empty() {
            "sin key"
        } else {
            "key✓"
        };
        let fo = if p.free_only { " · free-only" } else { "" };
        println!(
            "\n• {} [{}] {}{} · filtro: {} — {}",
            p.name,
            p.id,
            key,
            fo,
            p.free_filter.as_str(),
            p.base_url
        );
        for m in &p.models {
            let tag = if m.free { "free" } else { "pago" };
            let caps: Vec<&str> = m.caps.iter().map(|c| c.as_str()).collect();
            println!(
                "    - {}  [{} p{} c{} · {}]",
                m.name,
                tag,
                m.power,
                m.cost_scaled,
                caps.join("+")
            );
        }
    }
    if let Some(dm) = &cfg.default_model {
        println!("\nmodelo por defecto: {dm}");
    }
    println!("\nconfig propia: {}", config::own_config_path().display());
    Ok(())
}

fn print_route(cfg: &Config, task: &str, free_only: bool, cap: Option<Cap>) -> Result<()> {
    let clase = if ffi::router_is_power_task(task) {
        "potencia"
    } else {
        "rápida"
    };
    match cfg.route(task, &[], free_only || cfg.free_only, cap, false) {
        Some(c) => {
            let tag = if c.free { "FREE" } else { "pago" };
            let cap = cap
                .map(|c| format!(" · cap {}", c.as_str()))
                .unwrap_or_default();
            println!("tarea: {task} ({clase}{cap})");
            println!(
                "→ {}  [{tag}  power {}  costo {}]",
                c.label, c.power, c.cost_scaled
            );
        }
        None => println!("sin candidatos para `{task}` (configurá proveedores)"),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_ask(
    cfg: Config,
    cwd: PathBuf,
    task: String,
    free_only: bool,
    prompt: String,
    sandbox: bool,
    cap: Option<Cap>,
    images: Vec<image::Image>,
    model_override: Option<String>,
    prompt_cache: bool,
    context_chars: usize,
    skill_preload: Option<String>,
) -> Result<()> {
    use std::io::Write;
    let (tx, rx) = channel::<AgentEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let knowledge = Arc::new(Knowledge::discover(&cfg.knowledge));
    let skills = Arc::new(Registry::discover(
        &cwd,
        &cfg.skills,
        &cfg.skills_permissions,
        cfg.skills_autoinvoke,
    ));
    let mcp_hub = Arc::new(std::sync::Mutex::new(nucoder::mcp::McpHub::default()));
    let vision = !images.is_empty();
    let (perm_web, perm_read_outside, perm_terminal, perm_ftp, autopilot) = (
        cfg.perm_web,
        cfg.perm_read_outside,
        cfg.perm_terminal,
        cfg.perm_ftp,
        cfg.autopilot,
    );
    let agent = Agent {
        config: Arc::new(cfg),
        cwd: cwd.clone(),
        task,
        free_only,
        cancel,
        // One-shot no es interactivo: no puede pedir confirmación.
        confirm: false,
        sandbox,
        knowledge: knowledge.clone(),
        skills: skills.clone(),
        cap,
        vision,
        model_override,
        context_chars,
        terminal: None,
        auto_approve: true,
        autopilot,
        web: perm_web,
        read_outside: perm_read_outside,
        terminal_allowed: perm_terminal,
        ftp: None,
        ftp_allowed: perm_ftp,
        mcp_hub: mcp_hub.clone(),
    };
    let (prompt, mention_ctx, loaded, missing) = skills.expand_mentions(&prompt);
    if !loaded.is_empty() {
        eprintln!("· skills precargados: {}", loaded.join(", "));
    }
    if !missing.is_empty() {
        eprintln!("· skills no encontrados: {}", missing.join(", "));
    }
    let full = if mention_ctx.is_empty() {
        prompt.clone()
    } else {
        format!("{mention_ctx}\n\n---\n{prompt}")
    };
    let user_msg = agent::user_message(&full, &images);
    let mut system = agent::system_prompt(&cwd, &knowledge, &skills, false);
    if let Some(id) = &skill_preload {
        match skills.load(id) {
            Ok(body) => {
                system.push_str("\n\n");
                system.push_str(&body);
            }
            Err(e) => eprintln!("· --skill {id}: {e}"),
        }
    }
    let system = agent::system_value(&system, prompt_cache);
    let conv = agent::new_conversation_with(system, user_msg);
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
            // No debería llegar (en one-shot `confirm` está apagado), pero si
            // llega se rechaza por defecto.
            AgentEvent::Confirm { reply, .. } => {
                let _ = reply.send(false);
            }
            // One-shot sin interacción: se aprueba el plan para que continúe.
            AgentEvent::Plan { reply, .. } => {
                let _ = reply.send(true);
            }
            AgentEvent::Error(m) => eprintln!("\x1b[31m✗ {m}\x1b[0m"),
            // El modelo falló y se sacó del ruteo (en la CLI no se persiste).
            AgentEvent::Dead(id) => eprintln!("\x1b[90m· descartado: {id}\x1b[0m"),
            AgentEvent::Done { .. } => break,
        }
    }
    let _ = handle.join();
    println!();
    Ok(())
}

/// Carga la imagen de `--img <ruta>` (máx. 1 por turno).
fn load_images(args: &[String]) -> Result<Vec<image::Image>> {
    let paths = values_of(args, "--img");
    if paths.len() > 1 {
        eprintln!("· sólo se admite 1 imagen por turno; se usa la primera.");
    }
    match paths.first() {
        Some(p) => {
            let im = image::load(std::path::Path::new(p))
                .map_err(|e| anyhow::anyhow!("--img {p}: {e}"))?;
            Ok(vec![im])
        }
        None => Ok(Vec::new()),
    }
}

/// Prompt del subcomando `ask`, quitando los flags con valor.
fn ask_prompt(args: &[String]) -> String {
    const VALUED: [&str; 6] = [
        "--img",
        "--model",
        "--cap",
        "--task",
        "--cwd",
        "--context-chars",
    ];
    let mut out = Vec::new();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if VALUED.contains(&a.as_str()) {
            let _ = it.next();
            continue;
        }
        if VALUED.iter().any(|f| a.starts_with(&format!("{f}="))) {
            continue;
        }
        out.push(a.clone());
    }
    out.join(" ")
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
    println!();
    for l in banner::ART {
        println!("  {l}");
    }
    println!("  {}", banner::header());
    println!();
    println!("Uso:");
    println!("  nucoder                          Abre la TUI (chat + herramientas)");
    println!("  nucoder ask \"<pregunta>\"         Modo one-shot (imprime la respuesta)");
    println!("  nucoder --img <ruta> --ask \"…\"   One-shot con imagen (modelo con visión)");
    println!("  nucoder --route <task>           Muestra qué proveedor/modelo elegiría");
    println!("  nucoder --providers              Lista proveedores y modelos");
    println!("  nucoder models [--free] [--cap <c>] [--provider <id>]   Lista modelos");
    println!("  nucoder config                   Muestra la config (auto-aprobar, sandbox…)");
    println!("  nucoder config auto-approve on   No pedir confirmación (borrar/mover/bash)");
    println!("  nucoder config autopilot on      Tras aprobar el plan, ejecutar todo sin parar");
    println!("  nucoder config internet on|off · leer-fuera · terminales   Permisos del agente");
    println!("  nucoder skills                   Lista los skills (Claude/OpenCode/nativos)");
    println!("  nucoder skill <id>               Muestra el cuerpo de un skill");
    println!(
        "  nucoder cook <id|ruta> [--gourmet] [--json] [--stats]   Destila la habilidad (NuCook)"
    );
    println!("  nucoder mcp list                 Lista los servidores MCP configurados");
    println!("  nucoder mcp tools <id>          Herramientas que expone un server MCP");
    println!("  nucoder mcp call <id> <tool> [json]   Llama una herramienta MCP");
    println!("  nucoder provider add <id> [...]  Alta de proveedor (usa un preset)");
    println!("  nucoder provider presets         Presets conocidos (openrouter, nvidia, …)");
    println!("  nucoder provider remove <id>     Baja de proveedor propio");
    println!("  nucoder discover <id> [--save-free|--save-all]  Lista modelos del endpoint");
    println!("  nucoder refresh [id] [--reset-free]             Re-baja y reclasifica el catálogo");
    println!("  nucoder --version | --help");
    println!();
    println!("Flags:");
    println!("  --free            Sólo modelos gratuitos");
    println!("  --cap <c>         Restringe a una capacidad (text|code|vision|image|audio|video|embedding|guard)");
    println!(
        "  --img <ruta>      Adjunta una imagen (máx. 1 por turno; requiere modelo con visión)"
    );
    println!("  --model <id>      Fuerza un modelo (ignora el router; sin rotación)");
    println!("  --prompt-cache    Marca el system como cacheable (prompt caching del proveedor)");
    println!("  --context-chars <n>  Tope de contexto para la compactación (default 48000)");
    println!("  --skill <id>      Precarga un skill (ver `nucoder skills`) en el system");
    println!("  --task <t>        Tarea (complete|refactor|debug|explain|test|architect)");
    println!("  --cwd <path>      Raíz del proyecto (default: directorio actual)");
    println!("  --sandbox         Confina las tools de archivos al proyecto (default ON)");
    println!("  --no-sandbox      Permite rutas fuera del proyecto");
    println!("  --confirm         Pide confirmación antes de borrar/mover/bash (default ON)");
    println!("  --no-confirm      No pedir confirmación (equivale a auto-approve on)");
    println!();
    println!("Conocimiento local (KB) para `read_docs`: config `knowledge` o");
    println!("`NUCODER_KNOWLEDGE` (rutas separadas por ';'). SpASM se autodetecta.");
    println!("Skills (Claude/OpenCode/nativos): config `skills` o `NUCODER_SKILLS`.");
    println!();
    println!("Ejemplo (OpenRouter gratis):");
    println!("  nucoder provider add openrouter --api-key sk-or-...");
    println!("  nucoder discover openrouter --save-free");
    println!();
    println!("Un proveedor `free_only` sin modelos se autodescubre al arrancar.");
    println!();
    println!("Config propia: {}", config::own_config_path().display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, name: &str) -> client::ModelInfo {
        client::ModelInfo {
            id: id.into(),
            name: name.into(),
            raw: serde_json::json!(null),
        }
    }

    #[test]
    fn discovered_model_incluye_caps_y_free() {
        let v = discovered_model(&info("openai/whisper-large", "Whisper"), true);
        assert_eq!(v["free"], true);
        let caps = v["caps"].as_array().unwrap();
        assert!(caps.iter().any(|c| c == "audio"));
    }

    #[test]
    fn parsea_on_off() {
        assert!(parse_on_off("on").unwrap());
        assert!(parse_on_off("TRUE").unwrap());
        assert!(!parse_on_off("off").unwrap());
        assert!(!parse_on_off("no").unwrap());
        assert!(parse_on_off("quizá").is_err());
    }

    #[test]
    fn clasifica_gratis_por_filtro_del_proveedor() {
        let prov = config::Provider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            base_url: "https://x/v1".into(),
            api_key: String::new(),
            free_only: true,
            free_filter: FreeFilter::Auto,
            free_match: Vec::new(),
            models: Vec::new(),
        };
        let free = client::ModelInfo {
            id: "a/b:free".into(),
            name: "B".into(),
            raw: serde_json::json!(null),
        };
        assert!(model_free(&prov, &free));
        assert_eq!(
            caps_label("x/y", "modelo", &serde_json::json!(null)),
            "text"
        );
    }
}
