//! **Hub de skills**: buscar, instalar y administrar skills.
//!
//! - **Buscar**: filtra el catálogo descubierto por id, nombre o descripción.
//! - **Instalar**: desde un **directorio local** o desde un **repo git**
//!   (`owner/repo` o URL). Clona shallow, encuentra el skill y lo copia al
//!   destino compartido (`~/.agents/skills`), que NuCoder ya escanea.
//! - **Desinstalar**: borra el skill instalado (sólo dentro de la raíz del hub).
//!
//! Sin crates nuevas: `std` + `git` (proceso hijo) + `crate::proc::hidden`.
//! Se puede instalar desde GitHub/GitLab con `owner/repo` o cualquier URL de git.

use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::{Registry, Skill};

/// Raíz donde se instalan los skills “de la casa” (destino compartido que
/// NuCoder escanea: `~/.agents/skills`).
pub fn installed_root() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".agents").join("skills"))
        .unwrap_or_else(|| PathBuf::from(".agents/skills"))
}

/// Busca skills por subcadena en id, nombre o descripción (vacío = todos).
pub fn search<'a>(reg: &'a Registry, query: &str) -> Vec<&'a Skill> {
    let q = query.trim().to_lowercase();
    reg.skills()
        .iter()
        .filter(|s| {
            q.is_empty()
                || s.id.to_lowercase().contains(&q)
                || s.name.to_lowercase().contains(&q)
                || s.description.to_lowercase().contains(&q)
        })
        .collect()
}

/// Una entrada de un **catálogo remoto** (`index.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: String,
    pub description: String,
    /// `owner/repo` o URL para instalar (opcional).
    pub source: Option<String>,
    /// Ruta/carpeta del skill dentro del repo (opcional, para `--skill`).
    pub path: Option<String>,
}

/// Parsea un catálogo `index.json`:
/// `{ "skills": [ { "name", "description", "source", "path" } ] }`.
pub fn parse_catalog(json: &str) -> Vec<CatalogEntry> {
    let json = json.strip_prefix('\u{feff}').unwrap_or(json).trim();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let arr = v
        .get("skills")
        .or_else(|| v.get("entries"))
        .and_then(|a| a.as_array());
    let Some(arr) = arr else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let name = e.get("name").and_then(|n| n.as_str())?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let s = |k: &str| e.get(k).and_then(|x| x.as_str()).map(str::to_string);
            Some(CatalogEntry {
                name,
                description: s("description").unwrap_or_default(),
                source: s("source").or_else(|| s("repo")),
                path: s("path").or_else(|| s("skill")),
            })
        })
        .collect()
}

/// Filtra entradas de catálogo por subcadena (name/description).
pub fn filter_catalog(entries: &[CatalogEntry], query: &str) -> Vec<CatalogEntry> {
    let q = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|e| {
            q.is_empty()
                || e.name.to_lowercase().contains(&q)
                || e.description.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// Baja el crudo de un catálogo remoto (`curl`).
pub fn fetch_catalog(url: &str) -> Result<String, String> {
    let out = crate::proc::curl()
        .args(["-sSL", "--max-time", "30", url])
        .output()
        .map_err(|e| format!("no se pudo ejecutar curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Busca en un catálogo remoto (baja + parsea + filtra).
pub fn search_catalog(url: &str, query: &str) -> Result<Vec<CatalogEntry>, String> {
    let body = fetch_catalog(url)?;
    Ok(filter_catalog(&parse_catalog(&body), query))
}

/// Instala un skill desde un **directorio o archivo local** en `dest_root`.
/// Devuelve el id instalado.
pub fn install_from_dir(src: &Path, dest_root: &Path, force: bool) -> Result<String, String> {
    let id = skill_id_in(src).ok_or_else(|| {
        format!(
            "{} no parece un skill (falta SKILL.md o <id>.md)",
            src.display()
        )
    })?;
    let dest = dest_root.join(&id);
    if dest.exists() {
        if !force {
            return Err(format!(
                "ya está instalado `{id}` ({}); usá --force para reemplazar",
                dest.display()
            ));
        }
        fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    if src.is_file() {
        let name = src.file_name().unwrap_or_default();
        fs::copy(src, dest.join(name)).map_err(|e| format!("{}: {e}", src.display()))?;
    } else {
        copy_tree(src, &dest)?;
    }
    Ok(id)
}

/// Instala desde un repo git (`owner/repo` o URL). Clona shallow y copia el
/// skill al destino. `skill` elige la carpeta (`<skill>/SKILL.md`); sin él, el
/// primer `SKILL.md` que aparezca en las raíces típicas.
pub fn install_from_git(
    spec: &str,
    skill: Option<&str>,
    dest_root: &Path,
    force: bool,
) -> Result<String, String> {
    let url = normalize_git_url(spec);
    let tmp = std::env::temp_dir().join(format!("nucook-clone-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let out = crate::proc::hidden("git")
        .args(["clone", "--depth", "1", &url])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("no se pudo ejecutar git: {e}"))?;
    if !out.status.success() {
        let _ = fs::remove_dir_all(&tmp);
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git clone falló: {}", err.trim()));
    }
    let found = find_skill_dir(&tmp, skill);
    let result = match found {
        Some(dir) => install_from_dir(&dir, dest_root, force),
        None => Err(match skill {
            Some(s) => format!("no se encontró el skill `{s}` en {url}"),
            None => format!("no se encontró ningún SKILL.md en {url}"),
        }),
    };
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// Desinstala un skill del hub (sólo dentro de `dest_root`).
pub fn uninstall(id: &str, dest_root: &Path) -> Result<(), String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err(format!("id inválido: `{id}`"));
    }
    let dest = dest_root.join(id);
    if !dest.exists() {
        return Err(format!(
            "no está instalado `{id}` en {}",
            dest_root.display()
        ));
    }
    fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))
}

// ---------------------------------------------------------------------------
// Interno
// ---------------------------------------------------------------------------

/// Id de un skill local: nombre de la carpeta con `SKILL.md`, o el del `.md`.
fn skill_id_in(src: &Path) -> Option<String> {
    if src.is_dir() {
        if src.join("SKILL.md").is_file() {
            return Some(src.file_name()?.to_string_lossy().to_string());
        }
        // Un `<id>.md` suelto y sólo uno.
        let mds: Vec<PathBuf> = fs::read_dir(src)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            })
            .collect();
        if mds.len() == 1 {
            return mds[0].file_stem().map(|s| s.to_string_lossy().to_string());
        }
        return None;
    }
    src.file_stem().map(|s| s.to_string_lossy().to_string())
}

/// Raíces típicas donde puede vivir un skill dentro de un repo.
const SKILL_ROOTS: [&str; 4] = [
    ".agents/skills",
    ".opencode/skills",
    ".claude/skills",
    "skills",
];

fn find_skill_dir(repo: &Path, skill: Option<&str>) -> Option<PathBuf> {
    if let Some(s) = skill {
        for r in SKILL_ROOTS {
            let dir = repo.join(r).join(s);
            if dir.join("SKILL.md").is_file() {
                return Some(dir);
            }
            let flat = repo.join(r).join(format!("{s}.md"));
            if flat.is_file() {
                return Some(flat);
            }
        }
        // Búsqueda por cualquier profundidad.
        let mut hits = Vec::new();
        walk_skill_dirs(repo, &mut hits, 0);
        return hits
            .into_iter()
            .find(|d| d.file_name().map(|n| n == s).unwrap_or(false));
    }
    for r in SKILL_ROOTS {
        let mut hits = Vec::new();
        walk_skill_dirs(&repo.join(r), &mut hits, 0);
        if let Some(d) = hits.into_iter().next() {
            return Some(d);
        }
    }
    None
}

fn walk_skill_dirs(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if p.join("SKILL.md").is_file() {
            out.push(p);
        } else {
            walk_skill_dirs(&p, out, depth + 1);
        }
    }
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let rd = fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for e in rd.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "node_modules" || name == "target" {
            continue;
        }
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// `owner/repo` → `https://github.com/owner/repo`; URL o `git@…` → tal cual.
fn normalize_git_url(spec: &str) -> String {
    let s = spec.trim();
    if s.contains("://") || s.starts_with("git@") {
        return s.to_string();
    }
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        return format!("https://github.com/{s}");
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nucook-hub-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn escribe(p: &Path, body: &str) {
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn busca_por_id_nombre_o_descripcion() {
        use crate::skills::SkillSource;
        let root = temp_dir("search");
        escribe(
            &root.join("git-release/SKILL.md"),
            "---\nname: Git Release\ndescription: preparar versiones\n---\nx",
        );
        escribe(
            &root.join("rust-opt/SKILL.md"),
            "---\nname: Rust Optimizer\ndescription: performance\n---\nx",
        );
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root.clone())]);
        assert_eq!(search(&reg, "git").len(), 1);
        assert_eq!(search(&reg, "performance")[0].id, "rust-opt");
        assert_eq!(search(&reg, "").len(), 2);
        assert!(search(&reg, "zzz").is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn instala_desde_directorio_y_desinstala() {
        let src = temp_dir("src");
        let sk = src.join("demo");
        escribe(&sk.join("SKILL.md"), "---\ndescription: demo\n---\ncuerpo");
        escribe(&sk.join("references/ref.md"), "ref");
        let dest = temp_dir("dest");
        let id = install_from_dir(&sk, &dest, false).unwrap();
        assert_eq!(id, "demo");
        assert!(dest.join("demo/SKILL.md").is_file());
        assert!(dest.join("demo/references/ref.md").is_file());
        // Sin --force falla.
        assert!(install_from_dir(&sk, &dest, false).is_err());
        assert!(install_from_dir(&sk, &dest, true).is_ok());
        // Desinstalar.
        uninstall("demo", &dest).unwrap();
        assert!(!dest.join("demo").exists());
        assert!(uninstall("demo", &dest).is_err());
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn no_permite_desinstalar_ids_peligrosos() {
        let dest = temp_dir("safe");
        assert!(uninstall("../x", &dest).is_err());
        assert!(uninstall("a/b", &dest).is_err());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn normaliza_owner_repo() {
        assert_eq!(
            normalize_git_url("morluto/rea"),
            "https://github.com/morluto/rea"
        );
        assert_eq!(
            normalize_git_url("https://gitlab.com/a/b.git"),
            "https://gitlab.com/a/b.git"
        );
        assert_eq!(
            normalize_git_url("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
    }

    #[test]
    fn parsea_y_filtra_un_catalogo_remoto() {
        let json = r#"{"skills":[
            {"name":"git-release","description":"Preparar releases","source":"acme/skills","path":"git-release"},
            {"name":"rea","description":"Reverse engineer","repo":"morluto/rea","skill":"reverse-engineer-anything"},
            {"description":"sin nombre"}
        ]}"#;
        let all = parse_catalog(json);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].source.as_deref(), Some("acme/skills"));
        assert_eq!(all[0].path.as_deref(), Some("git-release"));
        assert_eq!(all[1].source.as_deref(), Some("morluto/rea"));
        assert_eq!(all[1].path.as_deref(), Some("reverse-engineer-anything"));
        let hits = filter_catalog(&all, "release");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "git-release");
        assert_eq!(filter_catalog(&all, "").len(), 2);
        assert!(parse_catalog("no es json").is_empty());
    }
}
