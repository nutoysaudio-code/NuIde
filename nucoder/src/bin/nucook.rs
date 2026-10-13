//! 🍪 `nucook` — la CLI de **NuCook**: cocina un skill (una habilidad) en su
//! forma enseñable, sin ruido humano, en galletitas listas para cualquier IA.
//!
//! ```text
//! nucook <id|ruta> [--gourmet] [--model <id>] [--refresh] [--json] [--stats] [--save]
//! nucook cook <id|ruta> [...]      (explícito)
//! nucook list                      recetas (skills) disponibles
//! nucook bake [--gourmet] [--refresh]   cocina TODAS y guarda el recetario
//! nucook show <id>                 muestra una receta guardada
//! nucook --version | --help
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use nucoder::config::Config;
use nucoder::cook;
use nucoder::hub;
use nucoder::skills::Registry;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("nucook {} · {} 🍪", env!("CARGO_PKG_VERSION"), cook::LANG);
        return Ok(());
    }
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }

    match args[0].as_str() {
        "list" | "ls" => list_recipes(),
        "catalog" | "cat" => catalog(),
        "search" | "find" => search(&args),
        "install" | "add" => install(&args),
        "uninstall" | "remove" | "rm" => uninstall(&args),
        "bake" => bake(&args),
        "cook-dir" | "dir" => cook_dir(&args),
        "show" => show(&args),
        "cook" => cook_one(&args[1..]),
        _ => cook_one(&args),
    }
}

/// `nucook search <texto> [--remote] [--catalog <url>]`: busca skills locales y,
/// con `--remote`, en un catálogo (`index.json`).
fn search(args: &[String]) -> Result<()> {
    let remote = args.iter().any(|a| a == "--remote" || a == "--web");
    let catalog = value_of(args, "--catalog").or_else(|| std::env::var("NUCOOK_CATALOG").ok());
    let mut parts = Vec::new();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "--catalog" {
            let _ = it.next();
            continue;
        }
        if a.starts_with('-') || catalog.as_deref() == Some(a.as_str()) {
            continue;
        }
        parts.push(a.clone());
    }
    let q = parts.join(" ");

    let cfg = Config::load();
    let reg = registry(&cfg);
    let hits = hub::search(&reg, &q);
    if hits.is_empty() {
        println!("(sin resultados locales para «{}»)", q.trim());
    } else {
        println!("{} local(es) para «{}»:", hits.len(), q.trim());
        for sk in hits {
            let d = sk.description.lines().next().unwrap_or("");
            println!(
                "  {:<30} [{}] {}",
                sk.id,
                sk.source.badge(),
                truncate(d, 70)
            );
        }
    }

    if remote {
        match catalog {
            Some(url) => match hub::search_catalog(&url, &q) {
                Ok(entries) if entries.is_empty() => {
                    println!("\n(sin resultados remotos en {url})")
                }
                Ok(entries) => {
                    println!("\n{} remoto(s) en {url}:", entries.len());
                    for e in entries {
                        println!("  {:<30} {}", e.name, truncate(&e.description, 70));
                        if let Some(src) = &e.source {
                            let skill = match &e.path {
                                Some(p) if !p.is_empty() => format!(" --skill {p}"),
                                _ => String::new(),
                            };
                            println!("      instalar:  nucook install {src}{skill}");
                        }
                    }
                }
                Err(e) => eprintln!("\ncatálogo: {e}"),
            },
            None => eprintln!(
                "\n· sin catálogo configurado: pasá --catalog <url> o seteá NUCOOK_CATALOG"
            ),
        }
    } else {
        println!("\nbuscar en la web:  nucook search <texto> --remote --catalog <url>");
        println!("instalar local:    nucook install <owner/repo> --skill <id>");
    }
    Ok(())
}

/// `nucook install <ruta|owner/repo|url> [--skill <id>] [--force]`.
fn install(args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    let skill = value_of(args, "--skill");
    let Some(spec) = args.get(1).filter(|a| !a.starts_with('-')) else {
        bail!("uso: nucook install <ruta|owner/repo|url> [--skill <id>] [--force]");
    };
    let root = hub::installed_root();
    let id = if Path::new(spec).exists() {
        hub::install_from_dir(Path::new(spec), &root, force)
    } else {
        eprintln!("· clonando {spec} …");
        hub::install_from_git(spec, skill.as_deref(), &root, force)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("🍪 instalado `{id}` en {}", root.join(&id).display());
    println!("   (NuCoder lo descubre al reiniciar el panel; `nucook list` para verlo)");
    Ok(())
}

/// `nucook uninstall <id>`: borra un skill instalado del hub.
fn uninstall(args: &[String]) -> Result<()> {
    let Some(id) = args.get(1).filter(|a| !a.starts_with('-')) else {
        bail!("uso: nucook uninstall <id>");
    };
    let root = hub::installed_root();
    hub::uninstall(id, &root).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("🗑  desinstalado `{id}`");
    Ok(())
}

/// `nucook catalog`: el catálogo que verá el agente (recetas + primers).
fn catalog() -> Result<()> {
    let cfg = Config::load();
    let reg = registry(&cfg);
    let text = cook::catalog_prompt(&reg);
    if text.trim().is_empty() {
        println!("(sin skills para anunciar)");
    } else {
        print!("{text}");
    }
    Ok(())
}

/// `nucook list`: las recetas (skills) que se pueden cocinar.
fn list_recipes() -> Result<()> {
    let cfg = Config::load();
    let reg = registry(&cfg);
    if reg.is_empty() {
        println!("no hay recetas (skills) disponibles.");
        println!("Se buscan en ~/.claude/skills, .agents/skills, .opencode/skills,");
        println!("~/.config/opencode/skills y SPASM_HOME/skills. Sumá rutas con la");
        println!("config `skills` o `NUCODER_SKILLS`.");
        return Ok(());
    }
    println!("{} receta(s) para cocinar:", reg.skills().len());
    for sk in reg.skills() {
        let d = sk.description.lines().next().unwrap_or("");
        println!(
            "  {:<30} [{}] {}",
            sk.id,
            sk.source.badge(),
            truncate(d, 70)
        );
    }
    println!("\ncociná una:  nucook <id>   ·   todas:  nucook bake");
    Ok(())
}

/// `nucook bake`: cocina **todas** las recetas y las guarda en el recetario.
fn bake(args: &[String]) -> Result<()> {
    let gourmet = args.iter().any(|a| a == "--gourmet" || a == "--semantic");
    let refresh = args.iter().any(|a| a == "--refresh");
    let model = value_of(args, "--model");
    let cfg = Config::load();
    let reg = registry(&cfg);
    if reg.is_empty() {
        bail!("no hay recetas para cocinar (probá `nucook list`)");
    }
    let mut ok = 0usize;
    let mut failed = 0usize;
    for sk in reg.skills() {
        let cooked = if gourmet {
            cook::cook_skill_gourmet(sk, &cfg, model.as_deref(), refresh)
        } else {
            cook::cook_skill(sk)
        };
        match cooked {
            Ok(t) => match cook::save_cookie(&t) {
                Ok(path) => {
                    println!(
                        "🍪 {:<30} {} ({})",
                        sk.id,
                        truncate(&path.display().to_string(), 60),
                        t.mode
                    );
                    ok += 1;
                }
                Err(e) => {
                    eprintln!("✗ {}: {e}", sk.id);
                    failed += 1;
                }
            },
            Err(e) => {
                eprintln!("✗ {}: {e}", sk.id);
                failed += 1;
            }
        }
    }
    println!(
        "\n{ok} receta(s) en el recetario: {}{}",
        cook::cookbook_dir().display(),
        if failed > 0 {
            format!(" · {failed} con error")
        } else {
            String::new()
        }
    );
    Ok(())
}

/// `nucook cook-dir <dir> [--gourmet] [--out <dir>]`: cocina **todos** los
/// documentos (`.md`/`.txt`) de un directorio al recetario (KB, docs, datasets).
fn cook_dir(args: &[String]) -> Result<()> {
    let gourmet = args.iter().any(|a| a == "--gourmet" || a == "--semantic");
    let refresh = args.iter().any(|a| a == "--refresh");
    let model = value_of(args, "--model");
    let out = value_of(args, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(cook::cookbook_dir);
    let Some(dir) = args.get(1).filter(|a| !a.starts_with('-')) else {
        bail!("uso: nucook cook-dir <dir> [--gourmet] [--out <dir>] [--refresh]");
    };
    let docs = cook::collect_docs(Path::new(dir), 500);
    if docs.is_empty() {
        bail!("no hay documentos (.md/.txt) en {dir}");
    }
    let cfg = Config::load();
    let mut ok = 0usize;
    let mut fail = 0usize;
    for p in &docs {
        let id = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "doc".to_string());
        let res = if gourmet {
            match std::fs::read_to_string(p) {
                Ok(text) => {
                    let body = nucoder::skills::strip_frontmatter(&text);
                    cook::gourmet_body(&id, p, &body, &cfg, model.as_deref(), refresh)
                }
                Err(e) => Err(format!("{e}")),
            }
        } else {
            cook::cook_path(p)
        };
        match res {
            Ok(t) => match cook::save_cookie_to(&out, &t) {
                Ok(_) => {
                    println!("🍪 {:<30} {}", t.id, t.mode);
                    ok += 1;
                }
                Err(e) => {
                    eprintln!("✗ {}: {e}", p.display());
                    fail += 1;
                }
            },
            Err(e) => {
                eprintln!("✗ {}: {e}", p.display());
                fail += 1;
            }
        }
    }
    println!(
        "\n{ok} documento(s) en {}{}",
        out.display(),
        if fail > 0 {
            format!(" · {fail} con error")
        } else {
            String::new()
        }
    );
    Ok(())
}

/// `nucook show <id>`: muestra una receta guardada.
fn show(args: &[String]) -> Result<()> {
    let Some(id) = args.get(1).filter(|a| !a.starts_with('-')) else {
        bail!("uso: nucook show <id>");
    };
    match cook::read_cookie(id) {
        Some(json) => {
            println!("{json}");
            Ok(())
        }
        None => bail!("no hay receta guardada `{id}` (probá `nucook bake`)"),
    }
}

/// Cocina un skill o un archivo suelto y lo imprime (o lo guarda con `--save`).
fn cook_one(args: &[String]) -> Result<()> {
    let json = args.iter().any(|a| a == "--json");
    let stats = args.iter().any(|a| a == "--stats");
    let save = args.iter().any(|a| a == "--save");
    let gourmet = args.iter().any(|a| a == "--gourmet" || a == "--semantic");
    let refresh = args.iter().any(|a| a == "--refresh");
    let model = value_of(args, "--model");
    let Some(arg) = args.first().filter(|a| !a.starts_with('-')) else {
        bail!("uso: nucook <id|ruta> [--gourmet] [--model <id>] [--refresh] [--json] [--stats] [--save]");
    };

    let cfg = Config::load();
    let teaching = if Path::new(arg).is_file() {
        let p = Path::new(arg);
        if gourmet {
            let text = std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{arg}: {e}"))?;
            let body = nucoder::skills::strip_frontmatter(&text);
            let id = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".to_string());
            cook::gourmet_body(&id, p, &body, &cfg, model.as_deref(), refresh)
                .map_err(|e| anyhow::anyhow!("{e}"))?
        } else {
            cook::cook_path(p).map_err(|e| anyhow::anyhow!("{e}"))?
        }
    } else {
        let reg = registry(&cfg);
        let sk = reg.by_id(arg).ok_or_else(|| {
            anyhow::anyhow!("no existe la receta `{arg}` (ni un archivo con esa ruta)")
        })?;
        if gourmet {
            cook::cook_skill_gourmet(sk, &cfg, model.as_deref(), refresh)
                .map_err(|e| anyhow::anyhow!("{e}"))?
        } else {
            cook::cook_skill(sk).map_err(|e| anyhow::anyhow!("{e}"))?
        }
    };

    if save {
        let path = cook::save_cookie(&teaching).map_err(|e| anyhow::anyhow!("{e}"))?;
        eprintln!("🍪 guardada en {}", path.display());
    }
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

fn registry(cfg: &Config) -> Registry {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Registry::discover(
        &cwd,
        &cfg.skills,
        &cfg.skills_permissions,
        cfg.skills_autoinvoke,
    )
}

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

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

fn print_help() {
    println!("🍪 nucook {} · {}", env!("CARGO_PKG_VERSION"), cook::LANG);
    println!("Cocina un skill (una habilidad) en su forma enseñable, sin ruido humano.");
    println!();
    println!("Uso:");
    println!("  nucook <id|ruta> [flags]        cocina y muestra");
    println!("  nucook cook <id|ruta> [flags]   ídem (explícito)");
    println!("  nucook list                     recetas (skills) disponibles");
    println!("  nucook catalog                  el catálogo que verá el agente");
    println!("  nucook search <texto>           busca skills (--remote --catalog <url>)");
    println!("  nucook install <repo|ruta>      instala un skill (--skill <id>, --force)");
    println!("  nucook uninstall <id>           borra un skill instalado");
    println!("  nucook bake [--gourmet]         cocina TODAS y guarda el recetario");
    println!("  nucook cook-dir <dir>           cocina todos los .md/.txt de un directorio");
    println!("  nucook show <id>                muestra una receta guardada");
    println!("  nucook --version | --help");
    println!();
    println!("Flags:");
    println!("  --gourmet        pase semántico con el modelo (cacheado)");
    println!("  --model <id>     fuerza el modelo del gourmet");
    println!("  --refresh        ignora la caché y recocina");
    println!("  --json           salida JSON");
    println!("  --stats          unidades / conservadas / ruido / duplicados");
    println!("  --save           guarda la receta en el recetario");
    println!();
    println!("Recetario: {}", cook::cookbook_dir().display());
}
