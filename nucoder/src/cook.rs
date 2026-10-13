//! **NuCook**: convierte una habilidad (skill) en su forma **enseñable**, sin
//! ruido humano y sin perder nada en silencio.
//!
//! Un skill es una **habilidad**. La meta no es comprimir ni inventar un idioma,
//! sino **enseñar la habilidad limpia**: qué es, cuándo aplica (y cuándo no),
//! qué logra, qué necesita, qué hace, qué conserva, cómo se prueba y con qué
//! ejemplo. Todo lo demás es ruido.
//!
//! El **ruido estructural** (reglas, encabezados de navegación, boilerplate de
//! licencias/versión, líneas repetidas) se quita de forma determinista. El ruido
//! **semántico** (cortesía, narración, supuestos implícitos) queda para un pase
//! aparte con un modelo.
//!
//! Dos guardianes (tests):
//!
//! 1. **No se pierde**: cada unidad con carga del origen queda en la forma
//!    enseñable o consta explícitamente como ruido/duplicado. Nada desaparece
//!    sin registro.
//! 2. **No hay ruido**: la salida no trae reglas/encabezados/boilerplate ni
//!    líneas duplicadas.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::caps::Cap;
use crate::config::Config;
use crate::skills::{strip_frontmatter, Registry, Skill};

/// Identificador del formato (versionado: un idioma que cambia no es un puente).
pub const LANG: &str = "nucook/v1";

/// Versión de la **gramática de cocción** gourmet: si cambia el prompt, cambia
/// esto para invalidar la caché (una cocción vieja no debe reusarse).
pub const GOURMET_VERSION: &str = "2";

/// Las partes de una **habilidad bien enseñada** (una relación por parte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// Cuándo aplica.
    When,
    /// Cuándo **no** aplica (anti-trigger).
    NotWhen,
    /// Qué logra (una línea).
    Purpose,
    /// Qué necesita tener/observar antes.
    Requires,
    /// Qué hace (pasos/transformaciones).
    Does,
    /// Qué conserva (invariantes, lo que no se puede romper).
    Keeps,
    /// Ruteo: condición → acción.
    Maps,
    /// Cómo se prueba que quedó bien.
    Proves,
    /// Ejemplos canónicos (si el método solo es ambiguo).
    Instances,
    /// Riesgos, límites y huecos conocidos.
    Notes,
    /// Lo que no se pudo clasificar (se conserva para no perder).
    Rest,
}

const N_PARTS: usize = 11;

impl PartKind {
    /// Etiqueta corta que ve el modelo (en su superficie conocida).
    pub fn label(self) -> &'static str {
        match self {
            PartKind::When => "when",
            PartKind::NotWhen => "not",
            PartKind::Purpose => "want",
            PartKind::Requires => "have",
            PartKind::Does => "do",
            PartKind::Keeps => "keep",
            PartKind::Maps => "map",
            PartKind::Proves => "prove",
            PartKind::Instances => "eg",
            PartKind::Notes => "risk",
            PartKind::Rest => "rest",
        }
    }

    fn key(self) -> &'static str {
        match self {
            PartKind::When => "when",
            PartKind::NotWhen => "not_when",
            PartKind::Purpose => "purpose",
            PartKind::Requires => "requires",
            PartKind::Does => "does",
            PartKind::Keeps => "keeps",
            PartKind::Maps => "maps",
            PartKind::Proves => "proves",
            PartKind::Instances => "instances",
            PartKind::Notes => "notes",
            PartKind::Rest => "rest",
        }
    }

    /// Por el **encabezado** de sección.
    fn from_heading(line: &str) -> Option<Self> {
        let h = line.trim_start_matches('#').trim().to_lowercase();
        if h.is_empty() {
            return None;
        }
        let has = |words: &[&str]| words.iter().any(|w| h.contains(w));
        // El orden importa: los anti-triggers antes que los triggers.
        if has(&[
            "no usar",
            "cuando no",
            "not use",
            "not to use",
            "when not",
            "don't use",
            "do not use",
            "skip",
            "avoid",
        ]) {
            return Some(PartKind::NotWhen);
        }
        if has(&["when", "trigger", "cuando", "use when", "applicab"]) {
            return Some(PartKind::When);
        }
        if has(&[
            "goal",
            "purpose",
            "objective",
            "objetivo",
            "propósito",
            "para qué",
        ]) {
            return Some(PartKind::Purpose);
        }
        if has(&[
            "constraint",
            "preserve",
            "restrict",
            "never",
            "must",
            "rule",
            "conserv",
            "no romper",
        ]) {
            return Some(PartKind::Keeps);
        }
        if has(&["verify", "valid", "test", "prove", "check", "verific"]) {
            return Some(PartKind::Proves);
        }
        if has(&["require", "input", "prereq", "need", "antes de", "necesit"]) {
            return Some(PartKind::Requires);
        }
        if has(&["map", "route", "routing", "target", "if the target"]) {
            return Some(PartKind::Maps);
        }
        if has(&["example", "usage", "ejemplo", "uza"]) {
            return Some(PartKind::Instances);
        }
        if has(&[
            "note", "risk", "limit", "warn", "caveat", "limitac", "riesgo",
        ]) {
            return Some(PartKind::Notes);
        }
        if has(&[
            "step", "workflow", "process", "how", "do", "action", "method", "paso",
        ]) {
            return Some(PartKind::Does);
        }
        None
    }

    /// Por el **contenido** de la línea (cuando la sección no lo dijo).
    /// Inverso de [`PartKind::key`] (para leer una receta guardada).
    fn from_key(k: &str) -> Option<Self> {
        match k {
            "when" => Some(PartKind::When),
            "not_when" => Some(PartKind::NotWhen),
            "purpose" => Some(PartKind::Purpose),
            "requires" => Some(PartKind::Requires),
            "does" => Some(PartKind::Does),
            "keeps" => Some(PartKind::Keeps),
            "maps" => Some(PartKind::Maps),
            "proves" => Some(PartKind::Proves),
            "instances" => Some(PartKind::Instances),
            "notes" => Some(PartKind::Notes),
            "rest" => Some(PartKind::Rest),
            _ => None,
        }
    }

    /// Por la etiqueta que devuelve el modelo (pase gourmet). Las **canónicas**
    /// las decide SpASM (`spasm/cook.spasm`); los sinónimos quedan en Rust.
    fn from_label(s: &str) -> Option<Self> {
        let l = s.trim().to_lowercase();
        if let Some(i) = crate::ffi::cook_label_kind(&l) {
            return Self::from_index(i);
        }
        match l.as_str() {
            "trigger" => Some(PartKind::When),
            "not_when" | "skip" => Some(PartKind::NotWhen),
            "goal" | "purpose" => Some(PartKind::Purpose),
            "need" | "requires" | "input" => Some(PartKind::Requires),
            "does" | "step" | "action" => Some(PartKind::Does),
            "preserve" | "constraint" => Some(PartKind::Keeps),
            "route" => Some(PartKind::Maps),
            "verify" | "test" => Some(PartKind::Proves),
            "example" | "for example" => Some(PartKind::Instances),
            "note" | "warning" | "limit" => Some(PartKind::Notes),
            "other" => Some(PartKind::Rest),
            _ => None,
        }
    }

    /// Índice canónico (0..10) → parte.
    fn from_index(i: usize) -> Option<Self> {
        Some(match i {
            0 => PartKind::When,
            1 => PartKind::NotWhen,
            2 => PartKind::Purpose,
            3 => PartKind::Requires,
            4 => PartKind::Does,
            5 => PartKind::Keeps,
            6 => PartKind::Maps,
            7 => PartKind::Proves,
            8 => PartKind::Instances,
            9 => PartKind::Notes,
            10 => PartKind::Rest,
            _ => return None,
        })
    }

    fn from_cue(line: &str) -> Option<Self> {
        let l = line.to_lowercase();
        let starts = |words: &[&str]| words.iter().any(|w| l.starts_with(w));
        if l.contains("->") || l.contains('→') {
            return Some(PartKind::Maps);
        }
        if starts(&[
            "skip ",
            "do not ",
            "don't ",
            "never use ",
            "avoid ",
            "no usar",
        ]) {
            return Some(PartKind::NotWhen);
        }
        if starts(&[
            "must ",
            "always ",
            "never ",
            "keep ",
            "preserve ",
            "no romper",
            "constraint",
            "required ",
        ]) {
            return Some(PartKind::Keeps);
        }
        if starts(&["verify ", "test ", "check ", "ensure ", "probar ", "valid"]) {
            return Some(PartKind::Proves);
        }
        if starts(&[
            "use ", "run ", "call ", "open ", "execute ", "usá ", "usar ",
        ]) {
            return Some(PartKind::Does);
        }
        None
    }
}

/// Una parte de la habilidad, con sus líneas.
#[derive(Debug, Clone)]
pub struct Part {
    pub kind: PartKind,
    pub lines: Vec<String>,
}

/// La **habilidad enseñada**: lo que se le da a cualquier IA.
#[derive(Debug, Clone)]
pub struct Teaching {
    pub id: String,
    /// De dónde salió (para poder volver al original; nada queda irrecuperable).
    pub source: PathBuf,
    pub source_hash: String,
    /// `deterministic` o `gourmet:<modelo>`.
    pub mode: String,
    pub parts: Vec<Part>,
    pub stats: Stats,
}

/// Metadatos de la cocción (para auditar que no se perdió ni sobró).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Unidades con carga vistas en el origen.
    pub units: usize,
    /// Unidades conservadas.
    pub kept: usize,
    /// Ruido estructural descartado.
    pub noise: usize,
    /// Duplicados colapsados.
    pub dup: usize,
}

impl Teaching {
    /// Render para el modelo: etiqueta + línea, una relación por línea.
    pub fn render_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "{} · {} · una relación por línea · fuente {} (sha {})\n",
            LANG,
            self.mode,
            self.source.display(),
            &self.source_hash[..self.source_hash.len().min(8)]
        ));
        s.push_str(&format!("skill {}\n", self.id));
        for p in &self.parts {
            for line in &p.lines {
                s.push_str(&format!("{:<6}{}\n", p.kind.label(), line));
            }
        }
        s
    }

    /// Render estructurado (Rust/UI/diff/caché).
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        for p in &self.parts {
            map.insert(p.kind.key().to_string(), json!(p.lines));
        }
        json!({
            "lang": LANG,
            "id": self.id,
            "mode": self.mode,
            "source": self.source.display().to_string(),
            "source_hash": self.source_hash,
            "stats": {
                "units": self.stats.units,
                "kept": self.stats.kept,
                "noise": self.stats.noise,
                "dup": self.stats.dup,
            },
            "parts": Value::Object(map),
        })
    }

    /// ¿Está vacía de contenido con carga?
    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.lines.is_empty())
    }
}

/// Cocina un skill descubierto.
pub fn cook_skill(sk: &Skill) -> Result<Teaching, String> {
    let body = sk.body()?;
    Ok(distill(&sk.id, &sk.entry, &body))
}

/// Cocina un archivo suelto (`.md`/`.toml`-libre con frontmatter opcional).
pub fn cook_path(path: &Path) -> Result<Teaching, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let body = strip_frontmatter(&text);
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "skill".to_string());
    Ok(distill(&id, path, &body))
}

/// Destila un cuerpo en la forma enseñable (determinista, sin modelo).
pub fn distill(id: &str, source: &Path, body: &str) -> Teaching {
    let mut buckets: [Vec<String>; N_PARTS] = Default::default();
    let mut stats = Stats::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut current = PartKind::Rest;

    for raw in body.lines() {
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Reglas horizontales: ruido de formato (no son unidades de contenido).
        // Fences de código y separadores de tabla también son formato.
        if is_rule(t) || t.starts_with("```") || t.starts_with("~~~") || is_table_sep(t) {
            continue;
        }
        // Encabezados: estructura de navegación; fijan la sección actual.
        if t.starts_with('#') {
            current = PartKind::from_heading(t).unwrap_or(PartKind::Rest);
            continue;
        }
        // Unidad con carga.
        stats.units += 1;
        if is_boilerplate(t) {
            stats.noise += 1;
            continue;
        }
        let clean = strip_marker(t).trim();
        if clean.is_empty() {
            stats.noise += 1;
            continue;
        }
        if !seen.insert(clean.to_lowercase()) {
            stats.dup += 1;
            continue;
        }
        let kind = if current != PartKind::Rest {
            current
        } else {
            PartKind::from_cue(clean).unwrap_or(PartKind::Rest)
        };
        buckets[kind as usize].push(clean.to_string());
        stats.kept += 1;
    }

    let order = part_order();
    let parts = order
        .iter()
        .map(|&k| Part {
            kind: k,
            lines: std::mem::take(&mut buckets[k as usize]),
        })
        .collect();

    Teaching {
        id: id.to_string(),
        source: source.to_path_buf(),
        source_hash: fnv1a(body.as_bytes()),
        mode: "deterministic".to_string(),
        parts,
        stats,
    }
}

fn part_order() -> [PartKind; N_PARTS] {
    [
        PartKind::When,
        PartKind::NotWhen,
        PartKind::Purpose,
        PartKind::Requires,
        PartKind::Does,
        PartKind::Keeps,
        PartKind::Maps,
        PartKind::Proves,
        PartKind::Instances,
        PartKind::Notes,
        PartKind::Rest,
    ]
}

fn build_parts(mut buckets: [Vec<String>; N_PARTS]) -> Vec<Part> {
    part_order()
        .iter()
        .map(|&k| Part {
            kind: k,
            lines: std::mem::take(&mut buckets[k as usize]),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pase gourmet (semántico, con el modelo)
// ---------------------------------------------------------------------------

/// Instrucción de sistema de la cocción semántica.
pub fn gourmet_system() -> &'static str {
    "Sos NuCook. Destilás un skill (una habilidad) en líneas `etiqueta relación`, una \
     por línea. Cada línea empieza con UNA etiqueta, y cada etiqueta tiene un \
     significado EXACTO:\n\
     - when  = condición para USAR la habilidad (trigger).\n\
     - not   = condición para NO usarla (exclusión).\n\
     - want  = qué logra (objetivo).\n\
     - have  = qué necesita (entradas/prerrequisitos).\n\
     - do    = pasos/transformaciones.\n\
     - keep  = invariantes que NO se pueden romper.\n\
     - map   = ruteo: `caso -> herramienta/acción`.\n\
     - prove = cómo se valida.\n\
     - eg    = ejemplo canónico.\n\
     - risk  = límite/riesgo/hueco.\n\
     - rest  = lo que no encaja en ninguna.\n\
     Reglas: sin encabezados, sin markdown, sin viñetas, sin negritas, sin \
     explicaciones ni comentarios; una relación por línea; NO resumas ni \
     parafrasees de más: conservá hechos, nombres de herramientas, condiciones y \
     números. Los supuestos implícitos se hacen EXPLÍCITOS. No inventes nada.\n\
     Ejemplo:\n\
     when  target is a shipped binary\n\
     not   target is a source repository\n\
     want  explain a feature from the artifact\n\
     have  local path to the target\n\
     map   asar -> analyze_javascript_application\n\
     keep  cite evidence ids\n\
     prove reopen the session and check state\n\
     risk  no native engine on windows"
}

/// Mensaje de usuario con la habilidad a cocinar.
pub fn gourmet_user(id: &str, body: &str) -> String {
    format!("SKILL {id}\n---\n{body}")
}

/// Parsea la salida del modelo (líneas `etiqueta relación`) a una `Teaching`.
pub fn parse_teaching(
    id: &str,
    source: &Path,
    source_hash: &str,
    mode: &str,
    response: &str,
) -> Teaching {
    let mut buckets: [Vec<String>; N_PARTS] = Default::default();
    let mut stats = Stats::default();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in response.lines() {
        let line = raw.trim().trim_start_matches(['-', '*', ' ', '\t']);
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        let (label, rest) = split_label(line);
        let kind = label
            .as_deref()
            .and_then(PartKind::from_label)
            .unwrap_or(PartKind::Rest);
        let text = rest.trim();
        if text.is_empty() {
            continue;
        }
        stats.units += 1;
        if !seen.insert(text.to_lowercase()) {
            stats.dup += 1;
            continue;
        }
        buckets[kind as usize].push(text.to_string());
        stats.kept += 1;
    }
    Teaching {
        id: id.to_string(),
        source: source.to_path_buf(),
        source_hash: source_hash.to_string(),
        mode: mode.to_string(),
        parts: build_parts(buckets),
        stats,
    }
}

/// Separa `etiqueta` del resto de la línea; `None` si no empieza con una etiqueta.
fn split_label(line: &str) -> (Option<String>, &str) {
    let sep = line.find([' ', ':', '\t']).unwrap_or(line.len());
    let token = line[..sep].trim();
    let rest = line[sep..].trim_start_matches([' ', ':', '\t']);
    if PartKind::from_label(token).is_some() {
        (Some(token.to_string()), rest)
    } else {
        (None, line)
    }
}

/// Elige proveedor/modelo para el pase gourmet.
fn pick_model(
    cfg: &Config,
    model_override: Option<&str>,
) -> Result<(String, String, String), String> {
    let cand = match model_override {
        Some(id) => cfg.find_candidate(id),
        None => cfg.route("architect", &[], cfg.free_only, Some(Cap::Text), false),
    };
    let c = cand.ok_or_else(|| {
        "no hay un modelo de texto disponible (configurá un proveedor con `nucoder provider add`)"
            .to_string()
    })?;
    let p = &cfg.providers[c.provider_idx];
    let m = &p.models[c.model_idx];
    Ok((p.base_url.clone(), p.api_key.clone(), m.id.clone()))
}

fn cache_dir() -> PathBuf {
    crate::config::own_config_path()
        .parent()
        .map(|p| p.join("cookies"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cache_file(hash: &str, mode: &str) -> PathBuf {
    let safe: String = mode
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    cache_dir().join(format!("{hash}-{safe}.cooked"))
}

/// Cocción semántica de un cuerpo ya leído (con caché por `hash` + modo).
pub fn gourmet_body(
    id: &str,
    source: &Path,
    body: &str,
    cfg: &Config,
    model_override: Option<&str>,
    refresh: bool,
) -> Result<Teaching, String> {
    let hash = fnv1a(body.as_bytes());
    let (base, key, model) = pick_model(cfg, model_override)?;
    let mode = format!("gourmet:{model}");
    // La caché depende del prompt (versión) además del modelo.
    let cache_mode = format!("{mode}@{GOURMET_VERSION}");
    let cached = if refresh {
        None
    } else {
        fs::read_to_string(cache_file(&hash, &cache_mode)).ok()
    };
    let response = match cached {
        Some(r) => r,
        None => {
            let r = crate::client::complete(
                &base,
                &key,
                &model,
                gourmet_system(),
                &gourmet_user(id, body),
                4096,
            )
            .map_err(|e| e.to_string())?;
            let path = cache_file(&hash, &cache_mode);
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(path, &r);
            r
        }
    };
    Ok(parse_teaching(id, source, &hash, &mode, &response))
}

/// Cocción semántica de un skill descubierto.
pub fn cook_skill_gourmet(
    sk: &Skill,
    cfg: &Config,
    model_override: Option<&str>,
    refresh: bool,
) -> Result<Teaching, String> {
    let body = sk.body()?;
    gourmet_body(&sk.id, &sk.entry, &body, cfg, model_override, refresh)
}

/// Extensiones que se cocinan al recorrer un directorio de documentos.
const DOC_EXT: [&str; 3] = ["md", "markdown", "txt"];

/// Recorre `dir` y devuelve los **documentos** cocinables (`.md`/`.markdown`/
/// `.txt`), hasta `max`, salteando generados y carpetas ocultas.
pub fn collect_docs(dir: &Path, max: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_docs(dir, &mut out, max, 0);
    out.sort();
    out.truncate(max);
    out
}

fn walk_docs(dir: &Path, out: &mut Vec<PathBuf>, max: usize, depth: usize) {
    if depth > 8 || out.len() >= max {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            walk_docs(&p, out, max, depth + 1);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| DOC_EXT.contains(&x.to_lowercase().as_str()))
            .unwrap_or(false)
        {
            out.push(p);
        }
        if out.len() >= max {
            return;
        }
    }
}

/// Directorio del **recetario** (junto a la caché).
pub fn cookbook_dir() -> PathBuf {
    crate::config::own_config_path()
        .parent()
        .map(|p| p.join("cookbook"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Guarda una receta como `<id>.cookie` (JSON) en `dir` y devuelve la ruta.
pub fn save_cookie_to(dir: &Path, t: &Teaching) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.cookie", sanitize_id(&t.id)));
    let json = serde_json::to_string_pretty(&t.to_json()).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Guarda una receta en el recetario del usuario.
pub fn save_cookie(t: &Teaching) -> Result<PathBuf, String> {
    save_cookie_to(&cookbook_dir(), t)
}

/// Lee una receta guardada del recetario (devuelve el JSON crudo).
pub fn read_cookie(id: &str) -> Option<String> {
    fs::read_to_string(cookbook_dir().join(format!("{}.cookie", sanitize_id(id)))).ok()
}

/// Carga una receta guardada y la reconstruye.
pub fn load_cookie(id: &str) -> Option<Teaching> {
    let text = read_cookie(id)?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(teaching_from_json(&v))
}

fn teaching_from_json(v: &Value) -> Teaching {
    let mut buckets: [Vec<String>; N_PARTS] = Default::default();
    if let Some(parts) = v.get("parts").and_then(Value::as_object) {
        for (k, val) in parts {
            if let Some(kind) = PartKind::from_key(k) {
                if let Some(arr) = val.as_array() {
                    buckets[kind as usize] = arr
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                }
            }
        }
    }
    let num =
        |path: &str| -> usize { v.pointer(path).and_then(Value::as_u64).unwrap_or(0) as usize };
    Teaching {
        id: v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        source: PathBuf::from(v.get("source").and_then(Value::as_str).unwrap_or("")),
        source_hash: v
            .get("source_hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        mode: v
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        parts: build_parts(buckets),
        stats: Stats {
            units: num("/stats/units"),
            kept: num("/stats/kept"),
            noise: num("/stats/noise"),
            dup: num("/stats/dup"),
        },
    }
}

/// Primer compacto de una receta (`want` + `when`, o la primera línea), corto.
pub fn primer_text(t: &Teaching, max: usize) -> String {
    let pick = |k: PartKind| {
        t.parts
            .iter()
            .find(|p| p.kind == k)
            .and_then(|p| p.lines.first())
            .cloned()
    };
    let mut bits = Vec::new();
    if let Some(w) = pick(PartKind::Purpose) {
        bits.push(format!("want {w}"));
    }
    if let Some(w) = pick(PartKind::When) {
        bits.push(format!("when {w}"));
    }
    if bits.is_empty() {
        if let Some(l) = t.parts.iter().flat_map(|p| p.lines.iter()).next() {
            bits.push(l.clone());
        }
    }
    truncate_chars(&bits.join(" · "), max)
}

/// Catálogo de skills para el prompt de sistema, **enriquecido con el recetario**:
/// si hay una receta cocida (`nucook bake`), se anuncia su primer; si no, la
/// descripción del skill (como antes).
pub fn catalog_prompt(reg: &Registry) -> String {
    let adv: Vec<&Skill> = reg.advertised().collect();
    if adv.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\n\nSKILLS disponibles: son procedimientos reutilizables (instrucciones de \
         Claude, OpenCode y nativos). Cuando uno sea relevante para la tarea, cargalo con \
         la herramienta `skill` (por id) y seguí sus pasos. No inventes skills que no \
         figuren acá.\n",
    );
    for sk in adv {
        let summary = match load_cookie(&sk.id) {
            Some(t) => primer_text(&t, 240),
            None => truncate_chars(sk.description.lines().next().unwrap_or(""), 240),
        };
        s.push_str(&format!("  - {} — {}: {}\n", sk.id, sk.name, summary));
    }
    s
}

fn truncate_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// ¿Es una regla horizontal? La determinan **SpASM** y un fallback en Rust.
fn is_rule(t: &str) -> bool {
    crate::ffi::cook_is_rule(t) || matches!(t, "---" | "***" | "___" | "----" | "===")
}

/// ¿Es una fila separadora de tabla Markdown (`|---|---|`)?
fn is_table_sep(t: &str) -> bool {
    let s = t.trim();
    s.starts_with('|')
        && s.contains('-')
        && s.chars()
            .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

/// Ruido **estructural** explícito (conservador: no toca contenido).
fn is_boilerplate(t: &str) -> bool {
    let l = t.to_lowercase();
    let l = l.trim_start_matches('>').trim_start();
    l.starts_with("copyright")
        || l.starts_with("license")
        || l.starts_with("licencia")
        || l.starts_with("author:")
        || l.starts_with("autor:")
        || l.starts_with("version:")
        || l.starts_with("versión:")
        || l.starts_with("©")
        || l == "table of contents"
        || l == "tabla de contenidos"
        || l == "índice"
}

/// Saca el marcador de lista/cita de una línea (`- `, `* `, `1. `, `> `).
fn strip_marker(s: &str) -> &str {
    let t = s.trim_start();
    for m in ["> ", "- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(m) {
            return r.trim_start();
        }
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let rest = &t[digits.len()..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return r.trim_start();
        }
    }
    t
}

/// FNV-1a 64-bit (hex) para content-address del origen. Sin crates.
fn fnv1a(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# Rust Optimizer

> Copyright 2026. License MIT.
> Version: 1.2.0

## When to use
- optimize a rust project
- the user asks for performance

## When not to use
- the project is not rust

## Goal
- improve performance

## Constraints
- preserve the public api
- preserve behavior
- never use unsafe code

## Steps
1. read the source files
2. reduce allocations
3. remove duplication
- read the source files

## Verify
- the project compiles
- tests pass

---

## Example
use with_capacity(0)

## Notes
warning: benchmarks can be noisy
"#;

    fn t() -> Teaching {
        distill("rust-optimizer", Path::new("/x/SKILL.md"), SAMPLE)
    }

    #[test]
    fn clasifica_las_partes_de_la_habilidad() {
        let t = t();
        let part = |k: PartKind| {
            t.parts
                .iter()
                .find(|p| p.kind == k)
                .map(|p| p.lines.clone())
                .unwrap_or_default()
        };
        assert_eq!(part(PartKind::Purpose), vec!["improve performance"]);
        assert!(part(PartKind::When).contains(&"optimize a rust project".to_string()));
        assert!(part(PartKind::NotWhen).contains(&"the project is not rust".to_string()));
        assert!(part(PartKind::Keeps).contains(&"never use unsafe code".to_string()));
        assert!(part(PartKind::Does).contains(&"reduce allocations".to_string()));
        assert!(part(PartKind::Proves).contains(&"tests pass".to_string()));
        assert!(part(PartKind::Instances).contains(&"use with_capacity(0)".to_string()));
        assert!(part(PartKind::Notes).contains(&"warning: benchmarks can be noisy".to_string()));
    }

    /// Guardián 1: **no se pierde** — cada unidad o se conserva o consta como
    /// ruido/duplicado. Nada desaparece en silencio.
    #[test]
    fn no_se_pierde_ninguna_unidad() {
        let t = t();
        assert_eq!(t.stats.units, t.stats.kept + t.stats.noise + t.stats.dup);
        // Y el duplicado ("read the source files") se colapsó.
        assert!(t.stats.dup >= 1);
    }

    /// Guardián 2: **no hay ruido** — sin reglas, sin boilerplate, sin duplicados.
    #[test]
    fn no_queda_ruido_estructural() {
        let text = t().render_text();
        assert!(!text.contains("Copyright"));
        assert!(!text.contains("License"));
        assert!(!text.contains("Version:"));
        assert!(!text.contains("---"));
        assert!(!text.contains("# Rust"));
        // El cuerpo no repite la misma línea dos veces.
        let mut seen = HashSet::new();
        for l in t().parts.iter().flat_map(|p| p.lines.iter()) {
            assert!(seen.insert(l.to_lowercase()), "duplicado: {l}");
        }
    }

    #[test]
    fn render_y_json_son_consistentes() {
        let t = t();
        let text = t.render_text();
        assert!(text.starts_with(LANG));
        assert!(text.contains("skill rust-optimizer"));
        assert!(text.contains("want  improve performance"));
        let j = t.to_json();
        assert_eq!(j["lang"], LANG);
        assert_eq!(j["id"], "rust-optimizer");
        assert_eq!(j["parts"]["purpose"][0], "improve performance");
        assert!(j["source_hash"].as_str().unwrap().len() == 16);
    }

    #[test]
    fn el_ruteo_se_detecta_por_flecha() {
        let body = "- asar -> analyze_javascript_application\n- native: open_binary\n";
        let t = distill("rea", Path::new("/x/SKILL.md"), body);
        let maps = t
            .parts
            .iter()
            .find(|p| p.kind == PartKind::Maps)
            .map(|p| p.lines.clone())
            .unwrap_or_default();
        assert!(maps.contains(&"asar -> analyze_javascript_application".to_string()));
    }

    #[test]
    fn parsea_la_salida_del_modelo() {
        let resp = "when  a rust project\nwant  improve performance\nmap  asar -> tool\n- prove tests pass\nzzz sin etiqueta\n";
        let t = parse_teaching("x", Path::new("/x"), "hash", "gourmet:m", resp);
        let part = |k: PartKind| {
            t.parts
                .iter()
                .find(|p| p.kind == k)
                .map(|p| p.lines.clone())
                .unwrap_or_default()
        };
        assert!(part(PartKind::When).contains(&"a rust project".to_string()));
        assert!(part(PartKind::Purpose).contains(&"improve performance".to_string()));
        assert!(part(PartKind::Maps).contains(&"asar -> tool".to_string()));
        // La viñeta `- ` se ignora y la línea sin etiqueta va a `rest` (no se pierde).
        assert!(part(PartKind::Proves).contains(&"tests pass".to_string()));
        assert!(part(PartKind::Rest).contains(&"zzz sin etiqueta".to_string()));
        assert_eq!(t.mode, "gourmet:m");
    }

    #[test]
    fn el_prompt_gourmet_pide_etiquetas_y_no_ruido() {
        let s = gourmet_system();
        assert!(s.contains("when  = "));
        assert!(s.contains("map   = "));
        assert!(s.contains("NO resumas"));
        assert!(s.contains("EXPLÍCITOS"));
        let u = gourmet_user("rea", "cuerpo");
        assert!(u.contains("SKILL rea"));
        assert!(u.ends_with("cuerpo"));
    }

    #[test]
    fn guarda_una_receta_en_el_recetario() {
        let dir = std::env::temp_dir().join(format!("nucook-cb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = save_cookie_to(&dir, &t()).unwrap();
        assert!(path.exists());
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("\"id\": \"rust-optimizer\""));
        assert!(json.contains("\"lang\": \"nucook/v1\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn una_receta_se_reconstruye_desde_json() {
        let t = t();
        let back = teaching_from_json(&t.to_json());
        assert_eq!(back.id, t.id);
        assert_eq!(back.mode, t.mode);
        let purpose = |x: &Teaching| {
            x.parts
                .iter()
                .find(|p| p.kind == PartKind::Purpose)
                .map(|p| p.lines.clone())
                .unwrap_or_default()
        };
        assert_eq!(purpose(&back), purpose(&t));
    }

    #[test]
    fn el_primer_prefiere_want_y_when() {
        let p = primer_text(&t(), 200);
        assert!(p.starts_with("want improve performance"), "primer: {p}");
        assert!(p.contains("when "));
    }

    #[test]
    fn las_etiquetas_canonicas_las_decide_spasm() {
        // SpASM (AOT) resuelve las etiquetas canónicas...
        assert_eq!(crate::ffi::cook_label_kind("want"), Some(2));
        assert_eq!(crate::ffi::cook_label_kind("rest"), Some(10));
        assert_eq!(crate::ffi::cook_label_kind("when"), Some(0));
        // ...los sinónimos (no canónicos) devuelven None y los cubre Rust.
        assert_eq!(crate::ffi::cook_label_kind("goal"), None);
        assert!(crate::ffi::cook_is_rule("---"));
        assert!(!crate::ffi::cook_is_rule("texto"));
        // El parser combina: SpASM para canónicas, Rust para sinónimos.
        assert_eq!(PartKind::from_label("want"), Some(PartKind::Purpose));
        assert_eq!(PartKind::from_label("goal"), Some(PartKind::Purpose));
    }

    #[test]
    fn recolecta_documentos_de_un_directorio() {
        let dir = std::env::temp_dir().join(format!("nucook-docs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let put = |rel: &str, body: &str| {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        };
        put("a.md", "# A");
        put("sub/b.txt", "b");
        put("c.png", "x");
        put("target/d.md", "no");
        let names: Vec<String> = collect_docs(&dir, 100)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"a.md".to_string()), "{names:?}");
        assert!(names.contains(&"b.txt".to_string()));
        assert!(!names.contains(&"c.png".to_string()));
        assert!(!names.contains(&"d.md".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }
}
