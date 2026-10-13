//! **Skills** del agente: instrucciones reutilizables para tareas concretas.
//!
//! Descubre y normaliza los tres formatos del ecosistema:
//!
//! - **Claude** — `SKILL.md` con *frontmatter* YAML (`~/.claude/skills`,
//!   `<proyecto>/.claude/skills`).
//! - **OpenCode** — `SKILL.md` o `<id>.md` en la raíz del origen
//!   (`~/.config/opencode/skills`, `.opencode/skills`, `.agents/skills`).
//! - **Nativo SpASM** — `skill.toml` + `README.md`
//!   (`SPASM_HOME/skills` o `F:\SpAMS-Oficial\skills`).
//!
//! Divulgación progresiva: al modelo sólo se le anuncia `id` + `description`
//! (ver [`Registry::catalog_prompt`]); el cuerpo se carga **a demanda** con la
//! herramienta `skill` (ver [`Registry::load`]). No hay embeddings: es un
//! índice de archivos, igual que la KB (`crate::knowledge`).
//!
//! Sin crates nuevas: el *frontmatter* YAML y el `skill.toml` se leen con un
//! parser mínimo y tolerante (si algo no parsea, el skill se indexa con lo que
//! sí se pudo leer).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Tope de archivos de apoyo que se le muestran al modelo al cargar un skill.
pub const MAX_RESOURCES: usize = 10;
/// Tope de la descripción que se anuncia en el prompt de sistema.
const MAX_DESCRIPTION: usize = 400;

/// De dónde salió el skill (para mostrarlo y para la precedencia).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// `~/.claude/skills`, `.claude/skills` (y `$CLAUDE_CONFIG_DIR/skills`).
    Claude,
    /// `~/.agents/skills`, `.agents/skills` (destino compartido; REA, Qwen…).
    Agents,
    /// `~/.config/opencode/skills`, `.opencode/skills`.
    OpenCode,
    /// Skills nativos de SpASM (`skill.toml` + `README.md`).
    Native,
    /// Rutas extra del config (`skills`) o de `NUCODER_SKILLS`.
    Config,
}

impl SkillSource {
    /// Etiqueta corta para el listado/CLI.
    pub fn badge(&self) -> &'static str {
        match self {
            SkillSource::Claude => "claude",
            SkillSource::Agents => "agents",
            SkillSource::OpenCode => "opencode",
            SkillSource::Native => "nativo",
            SkillSource::Config => "config",
        }
    }
}

/// Efecto de una regla de permiso sobre un skill (semántica de OpenCode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillEffect {
    /// Se anuncia y se carga sin preguntar (default).
    Allow,
    /// Se anuncia, pero cargarlo pide confirmación (si `confirm` está activo).
    Ask,
    /// No se anuncia y `load` lo rechaza.
    Deny,
}

impl SkillEffect {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkillEffect::Allow => "allow",
            SkillEffect::Ask => "ask",
            SkillEffect::Deny => "deny",
        }
    }

    /// Parsea `allow`/`ask`/`deny` (desconocido → `Allow`).
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" | "confirm" => SkillEffect::Ask,
            "deny" | "block" => SkillEffect::Deny,
            _ => SkillEffect::Allow,
        }
    }
}

/// Regla de permiso por id (glob), estilo OpenCode: la **última** que matchea
/// gana.
#[derive(Debug, Clone)]
pub struct SkillRule {
    pub id: String,
    pub effect: SkillEffect,
}

impl SkillRule {
    pub fn new(id: impl Into<String>, effect: SkillEffect) -> Self {
        SkillRule {
            id: id.into(),
            effect,
        }
    }
}

/// Un skill normalizado (los tres formatos caen acá).
#[derive(Debug, Clone)]
pub struct Skill {
    /// Id derivado de la ruta (sensible a mayúsculas); es lo que recibe la tool.
    pub id: String,
    /// Nombre para mostrar (frontmatter `name` o el id embellecido).
    pub name: String,
    /// Resumen que se anuncia al modelo (si está vacío, no se anuncia).
    pub description: String,
    /// Directorio base (para resolver rutas relativas del skill).
    pub dir: PathBuf,
    /// Documento de entrada (`SKILL.md`, `<id>.md` o el `entrypoint_doc`).
    pub entry: PathBuf,
    /// Archivos de apoyo, relativos a `dir` (hasta `MAX_RESOURCES`).
    pub resources: Vec<PathBuf>,
    /// Si es `false`, no se anuncia (pero se puede cargar por id).
    pub autoinvoke: bool,
    /// Herramientas que el skill declara (`allowed-tools`, informativo).
    pub allowed_tools: Vec<String>,
    pub source: SkillSource,
    /// `capabilities` del `skill.toml` nativo.
    pub capabilities: Vec<String>,
    /// `version` (frontmatter `metadata.version` o `skill.toml`).
    pub version: Option<String>,
}

impl Skill {
    /// ¿Tiene descripción? (sin descripción no se anuncia).
    pub fn has_description(&self) -> bool {
        !self.description.trim().is_empty()
    }

    /// Cuerpo del skill sin *frontmatter* (lo que enseña la habilidad).
    pub fn body(&self) -> Result<String, String> {
        let text = fs::read_to_string(&self.entry)
            .map_err(|e| format!("{}: {e}", self.entry.display()))?;
        Ok(strip_frontmatter(&text))
    }

    /// Resumen en una línea para el listado/CLI.
    pub fn line(&self) -> String {
        let desc = first_line(&self.description);
        format!("{:<28} [{}] {}", self.id, self.source.badge(), desc)
    }
}

/// Índice de skills descubiertos (deduplicados por id, con precedencia).
#[derive(Debug, Clone)]
pub struct Registry {
    skills: Vec<Skill>,
    /// Reglas de permiso por id (la última que matchea gana).
    rules: Vec<SkillRule>,
    /// Si es `false`, ningún skill se anuncia (pero se puede cargar por id).
    autoinvoke: bool,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            skills: Vec::new(),
            rules: Vec::new(),
            autoinvoke: true,
        }
    }
}

impl Registry {
    /// Descubre los skills del sistema para `cwd`, más las rutas `extra`
    /// (config `skills`) y `NUCODER_SKILLS`.
    ///
    /// Orden (menor → mayor precedencia; el último pisa al anterior por id),
    /// siguiendo el contrato de OpenCode:
    /// nativo → `.claude` (global y ancestros) → `.agents` → `opencode` global →
    /// `.opencode` (raíz → cwd) → `extra`/env.
    ///
    /// `rules` y `autoinvoke` vienen del config (`skills_permissions` y
    /// `skills_autoinvoke`).
    pub fn discover(cwd: &Path, extra: &[String], rules: &[SkillRule], autoinvoke: bool) -> Self {
        let home = dirs::home_dir();
        let mut sources: Vec<(SkillSource, PathBuf)> = Vec::new();

        // 1. Nativo SpASM (menor precedencia).
        for base in spasm_bases() {
            sources.push((SkillSource::Native, base.join("skills")));
        }
        // 2. Claude global (+ CLAUDE_CONFIG_DIR) y de proyecto.
        if let Some(home) = &home {
            sources.push((SkillSource::Claude, home.join(".claude").join("skills")));
        }
        if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
            if !dir.trim().is_empty() {
                sources.push((SkillSource::Claude, PathBuf::from(dir).join("skills")));
            }
        }
        push_project_sources(&mut sources, cwd, ".claude/skills", SkillSource::Claude);
        // 3. Agentes (destino compartido; REA instala acá).
        if let Some(home) = &home {
            sources.push((SkillSource::Agents, home.join(".agents").join("skills")));
        }
        push_project_sources(&mut sources, cwd, ".agents/skills", SkillSource::Agents);
        // 4. OpenCode global y de proyecto.
        if let Some(home) = &home {
            sources.push((
                SkillSource::OpenCode,
                home.join(".config").join("opencode").join("skills"),
            ));
        }
        push_project_sources(&mut sources, cwd, ".opencode/skills", SkillSource::OpenCode);
        // 5. Rutas extra del config y del entorno.
        for s in extra {
            if !s.trim().is_empty() {
                sources.push((SkillSource::Config, expand_home(s, home.as_deref())));
            }
        }
        if let Some(v) = std::env::var_os("NUCODER_SKILLS") {
            for p in std::env::split_paths(&v) {
                sources.push((SkillSource::Config, p));
            }
        }

        Self::from_sources(sources).with_permissions(rules, autoinvoke)
    }

    /// Construye el índice a partir de orígenes explícitos (útil en tests).
    pub fn from_sources(sources: Vec<(SkillSource, PathBuf)>) -> Self {
        let mut skills: Vec<Skill> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for (source, root) in sources {
            if !root.exists() {
                continue;
            }
            for mut sk in scan_source(source, &root) {
                sk.source = source;
                match index.get(&sk.id) {
                    // El origen de mayor precedencia reemplaza al anterior.
                    Some(&i) => skills[i] = sk,
                    None => {
                        index.insert(sk.id.clone(), skills.len());
                        skills.push(sk);
                    }
                }
            }
        }
        Registry {
            skills,
            rules: Vec::new(),
            autoinvoke: true,
        }
    }

    /// Aplica las reglas de permiso y el toggle global (builder).
    pub fn with_permissions(mut self, rules: &[SkillRule], autoinvoke: bool) -> Self {
        self.rules = rules.to_vec();
        self.autoinvoke = autoinvoke;
        self
    }

    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Busca por id exacto (sensible a mayúsculas, como OpenCode).
    pub fn by_id(&self, id: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.id == id)
    }

    /// Directorios de skills (raíces de lectura permitidas en el sandbox).
    pub fn dirs(&self) -> Vec<&Path> {
        self.skills.iter().map(|s| s.dir.as_path()).collect()
    }

    /// Efecto efectivo para `id`: la **última** regla que matchea gana; sin
    /// reglas, `Allow`.
    pub fn effect(&self, id: &str) -> SkillEffect {
        let mut effect = SkillEffect::Allow;
        for r in &self.rules {
            if rule_matches(&r.id, id) {
                effect = r.effect;
            }
        }
        effect
    }

    /// ¿Cargar este skill debe pedir confirmación? (`ask`)
    pub fn needs_confirmation(&self, id: &str) -> bool {
        self.effect(id) == SkillEffect::Ask
    }

    /// ¿Se anuncia al modelo? Descripción + `autoinvoke` (del skill y global) y
    /// permiso distinto de `deny`.
    pub fn advertises(&self, sk: &Skill) -> bool {
        self.autoinvoke
            && sk.autoinvoke
            && sk.has_description()
            && self.effect(&sk.id) != SkillEffect::Deny
    }

    /// Skills que se anuncian al modelo.
    pub fn advertised(&self) -> impl Iterator<Item = &Skill> {
        self.skills.iter().filter(|s| self.advertises(s))
    }

    /// Cuerpo del skill sin *frontmatter*, más el directorio base y los
    /// archivos de apoyo (como la tool `skill` de OpenCode).
    pub fn load(&self, id: &str) -> Result<String, String> {
        let sk = self.by_id(id).ok_or_else(|| {
            format!("no existe el skill `{id}` (pedí la herramienta sin id para listarlos)")
        })?;
        if self.effect(id) == SkillEffect::Deny {
            return Err(format!(
                "el skill `{id}` está bloqueado (`deny` en skills_permissions)"
            ));
        }
        let text =
            fs::read_to_string(&sk.entry).map_err(|e| format!("{}: {e}", sk.entry.display()))?;
        let (_, body) = split_frontmatter(&text);
        let mut out = body.trim_start_matches(['\n', '\r']).to_string();
        out.push_str(&format!("\n\n---\nDirectorio base: {}\n", sk.dir.display()));
        if !sk.resources.is_empty() {
            out.push_str("Archivos de apoyo (leelos con `read_file` si el skill te lo pide):\n");
            for r in &sk.resources {
                out.push_str(&format!("  - {}\n", r.display()));
            }
        }
        Ok(out)
    }

    /// Catálogo compacto (id + nombre + descripción) para el prompt de sistema.
    pub fn catalog_prompt(&self) -> String {
        let adv: Vec<&Skill> = self.advertised().collect();
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
            s.push_str(&format!(
                "  - {} — {}: {}\n",
                sk.id,
                sk.name,
                first_line_n(&sk.description, MAX_DESCRIPTION)
            ));
        }
        s
    }

    /// Expande las menciones `@<id>` del turno del usuario: saca la mención del
    /// texto y devuelve el **contexto** con el cuerpo de cada skill mencionado
    /// (para precargarlo sin esperar a la herramienta `skill`).
    ///
    /// Devuelve `(prompt_limpio, contexto, ids_cargados, ids_no_encontrados)`.
    /// Un skill con permiso `deny` no se precarga (cae en `no_encontrados`).
    pub fn expand_mentions(&self, prompt: &str) -> (String, String, Vec<String>, Vec<String>) {
        let chars: Vec<char> = prompt.chars().collect();
        let mut cleaned = String::new();
        let mut context = String::new();
        let mut loaded: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let mut prev_boundary = true;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '@' && prev_boundary {
                let start = i + 1;
                let mut j = start;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || chars[j] == '-' || chars[j] == '_')
                {
                    j += 1;
                }
                if j > start {
                    let id: String = chars[start..j].iter().collect();
                    match self.load(&id) {
                        Ok(body) => {
                            if !context.is_empty() {
                                context.push_str("\n\n");
                            }
                            context.push_str(&format!("## Skill `{id}`\n\n{body}"));
                            loaded.push(id);
                        }
                        Err(_) => missing.push(id),
                    }
                    i = j;
                    prev_boundary = false;
                    continue;
                }
            }
            cleaned.push(c);
            prev_boundary = c.is_whitespace();
            i += 1;
        }
        let mut cleaned = squeeze_spaces(&cleaned).trim().to_string();
        if cleaned.is_empty() && !loaded.is_empty() {
            cleaned = format!("Usá el skill `{}`.", loaded.join("`, `"));
        }
        (cleaned, context, loaded, missing)
    }
}

/// Colapsa espacios repetidos sin tocar los saltos de línea.
fn squeeze_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Descubrimiento
// ---------------------------------------------------------------------------

/// Bases candidatas del ecosistema SpASM (se agregan sólo si existen).
fn spasm_bases() -> Vec<PathBuf> {
    let mut bases = vec![PathBuf::from("F:/SpAMS-Oficial")];
    if let Some(home) = std::env::var_os("SPASM_HOME") {
        bases.push(PathBuf::from(home));
    }
    bases
}

/// Anade `<ancestro>/.claude/skills`… de la raíz hacia `cwd` (mayor precedencia
/// lo más cercano al proyecto actual).
fn push_project_sources(
    sources: &mut Vec<(SkillSource, PathBuf)>,
    cwd: &Path,
    rel: &str,
    source: SkillSource,
) {
    for ancestor in ancestors_root_first(cwd) {
        sources.push((source, ancestor.join(rel)));
    }
}

/// `cwd`, su padre, … hasta la raíz; devuelto raíz → cwd.
fn ancestors_root_first(cwd: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut cur = Some(cwd.to_path_buf());
    while let Some(p) = cur {
        cur = p.parent().map(Path::to_path_buf);
        v.push(p);
    }
    v.reverse();
    v
}

fn expand_home(s: &str, home: Option<&Path>) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(home) = home {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

fn scan_source(source: SkillSource, root: &Path) -> Vec<Skill> {
    match source {
        SkillSource::Native => scan_native(root),
        _ => scan_markdown(root),
    }
}

/// Formato Claude/OpenCode: `<id>.md` en la raíz y `<id>/SKILL.md` a cualquier
/// profundidad.
fn scan_markdown(root: &Path) -> Vec<Skill> {
    let mut out = Vec::new();
    // Archivos sueltos en la raíz: id = nombre de archivo sin extensión.
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || !has_ext(&p, "md") {
                continue;
            }
            let id = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone());
            if let Some(sk) = skill_from_markdown(id, p.clone(), root.to_path_buf(), Vec::new()) {
                out.push(sk);
            }
        }
    }
    // Directorios con SKILL.md (en cualquier profundidad).
    let mut dirs = Vec::new();
    walk_skill_dirs(root, &mut dirs, 0);
    for dir in dirs {
        let entry = dir.join("SKILL.md");
        let id = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let resources = list_resources(&dir, &entry);
        if let Some(sk) = skill_from_markdown(id, entry, dir.clone(), resources) {
            out.push(sk);
        }
    }
    out
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
            out.push(p); // no descendemos dentro de un skill
        } else {
            walk_skill_dirs(&p, out, depth + 1);
        }
    }
}

fn skill_from_markdown(
    id: String,
    entry: PathBuf,
    dir: PathBuf,
    resources: Vec<PathBuf>,
) -> Option<Skill> {
    let text = fs::read_to_string(&entry).ok()?;
    let front = split_frontmatter(&text)
        .0
        .map(parse_frontmatter)
        .unwrap_or_default();
    let name = front
        .name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| beautify_id(&id));
    let description = front.description.clone().unwrap_or_default();
    Some(Skill {
        id,
        name,
        description,
        dir,
        entry,
        resources,
        autoinvoke: front.autoinvoke.unwrap_or(true) && !front.disable_model_invocation,
        allowed_tools: front.allowed_tools,
        source: SkillSource::Claude,
        capabilities: Vec::new(),
        version: front.version,
    })
}

/// Formato nativo SpASM: `skill.toml` + `README.md` (o `entrypoint_doc`).
fn scan_native(root: &Path) -> Vec<Skill> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let manifest = dir.join("skill.toml");
        let readme = dir.join("README.md");
        if !manifest.is_file() && !readme.is_file() {
            continue;
        }
        let toml = fs::read_to_string(&manifest)
            .map(|t| parse_toml(&t))
            .unwrap_or_default();
        let entry = toml
            .entrypoint_doc
            .as_deref()
            .and_then(|rel| {
                // El `entrypoint_doc` suele venir del repo (`skills/<id>/…`):
                // nos quedamos con el nombre de archivo dentro del skill.
                let fname = rel.rsplit('/').next().unwrap_or(rel);
                let cand = dir.join(fname);
                cand.is_file().then_some(cand)
            })
            .or_else(|| readme.is_file().then(|| readme.clone()))
            .or_else(|| dir.join("SKILL.md").is_file().then(|| dir.join("SKILL.md")))
            .unwrap_or_else(|| readme.clone());
        if !entry.is_file() {
            continue;
        }
        let display = toml.name.clone().unwrap_or_else(|| beautify_id(&name));
        let description = toml
            .notes
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| first_readme_line(&entry))
            .unwrap_or_default();
        out.push(Skill {
            id: name,
            name: display,
            description,
            resources: list_resources(&dir, &entry),
            dir,
            entry,
            autoinvoke: true,
            allowed_tools: Vec::new(),
            source: SkillSource::Native,
            capabilities: toml.capabilities,
            version: toml.version,
        });
    }
    out
}

/// Archivos bajo `dir` (relativos), salteando el de entrada y los ocultos.
fn list_resources(dir: &Path, entry: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files(dir, dir, entry, &mut out, 0);
    out.sort();
    out.truncate(MAX_RESOURCES);
    out
}

fn collect_files(base: &Path, dir: &Path, entry: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 5 || out.len() >= MAX_RESOURCES * 4 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if p.is_dir() {
            collect_files(base, &p, entry, out, depth + 1);
        } else if p != *entry {
            if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.to_path_buf());
            }
        }
    }
}

fn first_readme_line(entry: &Path) -> Option<String> {
    let text = fs::read_to_string(entry).ok()?;
    let (_, body) = split_frontmatter(&text);
    body.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Parsers (sin crates): frontmatter YAML mínimo y skill.toml mínimo
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Front {
    name: Option<String>,
    description: Option<String>,
    /// `None` = no declarado (default `true`).
    autoinvoke: Option<bool>,
    disable_model_invocation: bool,
    allowed_tools: Vec<String>,
    version: Option<String>,
}

/// Cuerpo sin *frontmatter* (lo que enseña la habilidad), ya recortado a la derecha.
pub fn strip_frontmatter(text: &str) -> String {
    split_frontmatter(text)
        .1
        .trim_start_matches(['\n', '\r'])
        .to_string()
}

/// Separa el *frontmatter* (`---…---` al inicio) del cuerpo.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(after) = t.strip_prefix("---") else {
        return (None, text);
    };
    let after = after.strip_prefix('\r').unwrap_or(after);
    let Some(rest) = after.strip_prefix('\n') else {
        return (None, text);
    };
    let mut pos = 0usize;
    let mut fm_end = None;
    for line in rest.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.trim_end() == "---" {
            fm_end = Some((pos, pos + line.len()));
            break;
        }
        pos += line.len();
    }
    match fm_end {
        Some((_, end)) => (Some(&rest[..end]), &rest[end..]),
        None => (None, text),
    }
}

fn parse_frontmatter(fm: &str) -> Front {
    let mut f = Front::default();
    let mut collect_tools = false;
    let lines: Vec<&str> = fm.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end_matches('\r');
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();

        // Ítems de una lista en bloque (`- Read`).
        if collect_tools && indent > 0 && trimmed.starts_with('-') {
            let v = unquote(trimmed[1..].trim());
            if !v.is_empty() {
                f.allowed_tools.push(v.to_string());
            }
            continue;
        }
        collect_tools = false;

        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon].trim();

        // `clave: >` (plegado) o `clave: |` (literal): el valor son las líneas
        // siguientes más indentadas. Es lo que usan muchos `SKILL.md`
        // (caveman/ponytail) para descripciones multilínea.
        let value: String = match block_scalar_kind(trimmed[colon + 1..].trim()) {
            Some(folded) => {
                let block = take_block(&lines, &mut i, indent);
                if folded {
                    fold_block(&block)
                } else {
                    block.join("\n")
                }
            }
            None => unquote(trimmed[colon + 1..].trim()).to_string(),
        };
        let value = value.trim();

        if indent > 0 {
            // Dentro de `metadata:`.
            match key {
                "opencode/autoinvoke" | "autoinvoke" => f.autoinvoke = Some(parse_bool(value)),
                "version" if f.version.is_none() => f.version = nonempty(value).map(str::to_string),
                _ => {}
            }
            continue;
        }

        match key {
            "name" => f.name = nonempty(value).map(str::to_string),
            "description" => f.description = nonempty(value).map(str::to_string),
            "version" if f.version.is_none() => f.version = nonempty(value).map(str::to_string),
            "disable-model-invocation" => f.disable_model_invocation = parse_bool(value),
            "autoinvoke" => f.autoinvoke = Some(parse_bool(value)),
            "allowed-tools" => {
                let items = inline_list(value);
                if items.is_empty() {
                    collect_tools = true;
                } else {
                    f.allowed_tools = items;
                }
            }
            _ => {}
        }
    }
    f
}

/// Detecta un *block scalar* de YAML: `>`, `>-`, `>+`, `|`, `|-`, `|+`.
/// Devuelve `Some(true)` si es plegado (`>`), `Some(false)` si es literal (`|`).
fn block_scalar_kind(value: &str) -> Option<bool> {
    let mut chars = value.chars();
    match chars.next()? {
        '>' => Some(true),
        '|' => Some(false),
        _ => None,
    }
}

/// Junta las líneas de un *block scalar*: salta la indentación común, descarta
/// las líneas en blanco finales.
fn take_block(lines: &[&str], i: &mut usize, base_indent: usize) -> Vec<String> {
    let mut block: Vec<String> = Vec::new();
    let mut common: Option<usize> = None;
    while *i < lines.len() {
        let l = lines[*i].trim_end_matches('\r');
        if l.trim().is_empty() {
            block.push(String::new());
            *i += 1;
            continue;
        }
        let li = l.len() - l.trim_start().len();
        if li <= base_indent {
            break;
        }
        let cut = common.map(|c| c.min(li)).unwrap_or(li);
        common = Some(cut);
        block.push(l[cut..].to_string());
        *i += 1;
    }
    while block.last().is_some_and(|s| s.trim().is_empty()) {
        block.pop();
    }
    block
}

/// Plegado YAML (`>`): líneas consecutivas se unen con espacio; las líneas en
/// blanco se vuelven saltos de línea.
fn fold_block(lines: &[String]) -> String {
    let mut out = String::new();
    let mut prev_blank = true;
    for l in lines {
        if l.trim().is_empty() {
            out.push('\n');
            prev_blank = true;
        } else {
            if !prev_blank {
                out.push(' ');
            }
            out.push_str(l);
            prev_blank = false;
        }
    }
    out
}

#[derive(Debug, Default)]
struct Toml {
    name: Option<String>,
    version: Option<String>,
    entrypoint_doc: Option<String>,
    notes: Option<String>,
    capabilities: Vec<String>,
}

fn parse_toml(text: &str) -> Toml {
    let mut t = Toml::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        i += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else {
            continue;
        };
        let key = line[..eq].trim();
        let mut val = line[eq + 1..].trim().to_string();
        // Array multilínea: seguir juntando hasta balancear los corchetes.
        if val.starts_with('[') && !balanced(&val) {
            while i < lines.len() {
                val.push(' ');
                val.push_str(lines[i].trim());
                i += 1;
                if balanced(&val) {
                    break;
                }
            }
        }
        match key {
            "name" => t.name = nonempty(unquote(&val)).map(str::to_string),
            "version" => t.version = nonempty(unquote(&val)).map(str::to_string),
            "entrypoint_doc" => t.entrypoint_doc = nonempty(unquote(&val)).map(str::to_string),
            "notes" => t.notes = nonempty(unquote(&val)).map(str::to_string),
            "capabilities" => t.capabilities = toml_array(&val),
            _ => {}
        }
    }
    t
}

fn balanced(s: &str) -> bool {
    s.matches('[').count() == s.matches(']').count()
}

fn toml_array(val: &str) -> Vec<String> {
    let inner = val.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|s| unquote(s.trim()).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn inline_list(val: &str) -> Vec<String> {
    let inner = val.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|s| unquote(s.trim()).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(rest) = s.strip_prefix(q) {
            if let Some(inner) = rest.strip_suffix(q) {
                return inner;
            }
        }
    }
    s
}

fn parse_bool(s: &str) -> bool {
    matches!(
        unquote(s).trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1"
    )
}

fn nonempty(s: &str) -> Option<&str> {
    let s = s.trim();
    if s.is_empty() || s == "null" || s == "~" {
        None
    } else {
        Some(s)
    }
}

/// ¿El glob de una regla matchea el id? (`*` matchea todo; también vale exacto.)
fn rule_matches(glob: &str, id: &str) -> bool {
    let g = glob.trim();
    g == "*" || g == id || crate::tools::glob_match(g, id)
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

fn beautify_id(id: &str) -> String {
    let s = id.replace(['-', '_'], " ");
    let s = s.trim();
    if s.is_empty() {
        id.to_string()
    } else {
        s.to_string()
    }
}

fn first_line(s: &str) -> String {
    first_line_n(s, 120)
}

fn first_line_n(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "nucoder-skills-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, body: &str) {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn parsea_frontmatter_con_metadata() {
        let fm = "name: Git Release\ndescription: Preparar versión\nmetadata:\n  opencode/autoinvoke: false\n  version: \"40\"\n";
        let f = parse_frontmatter(fm);
        assert_eq!(f.name.as_deref(), Some("Git Release"));
        assert_eq!(f.description.as_deref(), Some("Preparar versión"));
        assert_eq!(f.autoinvoke, Some(false));
        assert_eq!(f.version.as_deref(), Some("40"));
    }

    #[test]
    fn parsea_descripcion_multilinea_plegada_y_literal() {
        // `description: >` (plegado): las líneas se unen con espacio.
        let fm = "name: caveman\ndescription: >\n  Terse caveman voice: answer first, fluff gone.\n  Use for \"caveman mode\", \"be brief\".\nlicense: MIT\n";
        let f = parse_frontmatter(fm);
        assert_eq!(
            f.description.as_deref(),
            Some("Terse caveman voice: answer first, fluff gone. Use for \"caveman mode\", \"be brief\".")
        );
        // `description: |` (literal): se conservan los saltos.
        let fm = "description: |\n  linea 1\n  linea 2\nautoinvoke: false\n";
        let f = parse_frontmatter(fm);
        assert_eq!(f.description.as_deref(), Some("linea 1\nlinea 2"));
        assert_eq!(f.autoinvoke, Some(false));
        // Un valor normal sigue andando.
        let f = parse_frontmatter("description: una sola linea\n");
        assert_eq!(f.description.as_deref(), Some("una sola linea"));
    }

    #[test]
    fn frontmatter_separa_cuerpo() {
        let text = "---\nname: X\ndescription: Y\n---\n\n# Título\ncuerpo\n";
        let (fm, body) = split_frontmatter(text);
        assert!(fm.unwrap().contains("name: X"));
        assert!(body.starts_with("\n# Título") || body.starts_with("# Título"));
        assert!(body.contains("cuerpo"));
        // Sin frontmatter no se toca.
        let (fm, body) = split_frontmatter("# solo cuerpo\n");
        assert!(fm.is_none());
        assert_eq!(body, "# solo cuerpo\n");
    }

    #[test]
    fn descubre_skill_de_directorio_con_recursos() {
        let root = temp_dir("dir");
        write(
            &root.join("git-release/SKILL.md"),
            "---\nname: Git Release\ndescription: Preparar release\n---\n\nPasos:\n1. leer references/policy.md\n",
        );
        write(&root.join("git-release/references/policy.md"), "política");
        write(&root.join("git-release/scripts/changelog.ts"), "// x");
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root.clone())]);
        assert_eq!(reg.skills().len(), 1);
        let sk = reg.by_id("git-release").unwrap();
        assert_eq!(sk.name, "Git Release");
        assert!(sk.resources.iter().any(|r| r.ends_with("policy.md")));
        let body = reg.load("git-release").unwrap();
        assert!(!body.contains("name: Git Release"));
        assert!(body.contains("Pasos"));
        assert!(body.contains("Directorio base"));
        assert!(body.contains("policy.md"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn descubre_archivo_plano_y_usa_el_nombre_como_id() {
        let root = temp_dir("flat");
        write(
            &root.join("review.md"),
            "---\nname: Review\ndescription: Revisar código\n---\nhacé la review\n",
        );
        let reg = Registry::from_sources(vec![(SkillSource::Claude, root.clone())]);
        assert_eq!(reg.by_id("review").unwrap().name, "Review");
        assert_eq!(reg.skills().len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn descubre_skill_nativo_toml() {
        let root = temp_dir("native");
        write(
            &root.join("spasm-architect/skill.toml"),
            "name = \"spasm-architect\"\nversion = \"0.1.0\"\nentrypoint_doc = \"skills/spasm-architect/README.md\"\n\ncapabilities = [\n  \"spasm.explain\",\n  \"spasm.write_code\",\n]\n\nnotes = \"KB del lenguaje SpASM\"\n",
        );
        write(
            &root.join("spasm-architect/README.md"),
            "# SpASM\n\nDetalle.",
        );
        let reg = Registry::from_sources(vec![(SkillSource::Native, root.clone())]);
        let sk = reg.by_id("spasm-architect").unwrap();
        assert_eq!(sk.source, SkillSource::Native);
        assert_eq!(sk.description, "KB del lenguaje SpASM");
        assert!(sk.capabilities.iter().any(|c| c == "spasm.write_code"));
        assert!(sk.entry.ends_with("README.md"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn precedencia_por_id_pisa_el_de_menor() {
        let low = temp_dir("low");
        let high = temp_dir("high");
        write(
            &low.join("x/SKILL.md"),
            "---\nname: Viejo\ndescription: viejo\n---\nv\n",
        );
        write(
            &high.join("x/SKILL.md"),
            "---\nname: Nuevo\ndescription: nuevo\n---\nn\n",
        );
        let reg = Registry::from_sources(vec![
            (SkillSource::Claude, low.clone()),
            (SkillSource::OpenCode, high.clone()),
        ]);
        assert_eq!(reg.skills().len(), 1);
        assert_eq!(reg.by_id("x").unwrap().name, "Nuevo");
        let _ = fs::remove_dir_all(&low);
        let _ = fs::remove_dir_all(&high);
    }

    #[test]
    fn solo_se_anuncian_los_que_tienen_descripcion_y_autoinvoke() {
        let root = temp_dir("adv");
        write(
            &root.join("con/SKILL.md"),
            "---\ndescription: sirve\n---\nx\n",
        );
        write(
            &root.join("sin-desc/SKILL.md"),
            "---\nname: sin desc\n---\nx\n",
        );
        write(
            &root.join("oculto/SKILL.md"),
            "---\ndescription: oculto\nmetadata:\n  opencode/autoinvoke: false\n---\nx\n",
        );
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root.clone())]);
        assert_eq!(reg.skills().len(), 3);
        let adv: Vec<&str> = reg.advertised().map(|s| s.id.as_str()).collect();
        assert_eq!(adv, vec!["con"]);
        assert!(reg.catalog_prompt().contains("con — "));
        assert!(!reg.catalog_prompt().contains("oculto"));
        // El oculto igual se puede cargar por id.
        assert!(reg.load("oculto").is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_de_id_inexistente_es_error() {
        let reg = Registry::default();
        assert!(reg.load("nope").is_err());
        assert!(reg.is_empty());
        assert!(reg.catalog_prompt().is_empty());
    }

    #[test]
    fn parsea_bool_toml_y_unquote() {
        assert!(parse_bool("true"));
        assert!(parse_bool("\"yes\""));
        assert!(!parse_bool("false"));
        assert_eq!(unquote("\"hola\""), "hola");
        assert_eq!(toml_array("[ \"a\", \"b\", ]"), vec!["a", "b"]);
    }

    #[test]
    fn deny_oculta_y_rechaza_la_carga() {
        let root = temp_dir("deny");
        write(
            &root.join("secreto/SKILL.md"),
            "---\ndescription: no\n---\nx\n",
        );
        write(
            &root.join("publico/SKILL.md"),
            "---\ndescription: sí\n---\nx\n",
        );
        let rules = vec![SkillRule::new("secreto", SkillEffect::Deny)];
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root.clone())])
            .with_permissions(&rules, true);
        let adv: Vec<&str> = reg.advertised().map(|s| s.id.as_str()).collect();
        assert_eq!(adv, vec!["publico"]);
        assert!(!reg.catalog_prompt().contains("secreto"));
        let err = reg.load("secreto").unwrap_err();
        assert!(err.contains("deny"), "error: {err}");
        assert!(reg.load("publico").is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ask_pide_confirmacion_pero_se_anuncia() {
        let root = temp_dir("ask");
        write(&root.join("raro/SKILL.md"), "---\ndescription: d\n---\nx\n");
        let rules = vec![SkillRule::new("raro", SkillEffect::Ask)];
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root.clone())])
            .with_permissions(&rules, true);
        assert!(reg.advertised().any(|s| s.id == "raro"));
        assert!(reg.needs_confirmation("raro"));
        assert!(!reg.needs_confirmation("otro"));
        // `ask` no bloquea la carga (la confirmación la maneja el agente).
        assert!(reg.load("raro").is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn la_ultima_regla_que_matchea_gana_y_admite_globs() {
        let rules = vec![
            SkillRule::new("*", SkillEffect::Allow),
            SkillRule::new("interno-*", SkillEffect::Allow),
            SkillRule::new("interno-*", SkillEffect::Deny),
        ];
        let reg = Registry::default().with_permissions(&rules, true);
        assert_eq!(reg.effect("interno-x"), SkillEffect::Deny);
        assert_eq!(reg.effect("publico"), SkillEffect::Allow);
        // El toggle global apaga todos los anuncios.
        let off = Registry::default().with_permissions(&rules, false);
        assert!(!off.advertises(&Skill {
            id: "x".into(),
            name: "x".into(),
            description: "d".into(),
            dir: PathBuf::new(),
            entry: PathBuf::new(),
            resources: Vec::new(),
            autoinvoke: true,
            allowed_tools: Vec::new(),
            source: SkillSource::Config,
            capabilities: Vec::new(),
            version: None,
        }));
    }

    #[test]
    fn parsea_efecto_de_permiso() {
        assert_eq!(SkillEffect::parse("deny"), SkillEffect::Deny);
        assert_eq!(SkillEffect::parse("ASK"), SkillEffect::Ask);
        assert_eq!(SkillEffect::parse("lo-que-sea"), SkillEffect::Allow);
        assert_eq!(SkillEffect::Deny.as_str(), "deny");
    }

    #[test]
    fn menciona_un_skill_y_lo_precarga() {
        let root = temp_dir("mention");
        write(
            &root.join("git-release/SKILL.md"),
            "---\ndescription: release\n---\nPasos del release.",
        );
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root.clone())]);
        let (prompt, ctx, loaded, missing) = reg.expand_mentions("hacé @git-release ahora");
        assert_eq!(prompt, "hacé ahora");
        assert_eq!(loaded, vec!["git-release"]);
        assert!(missing.is_empty());
        assert!(ctx.contains("Pasos del release."));
        // Sólo la mención: se arma un prompt de fallback.
        let (prompt, _, loaded, _) = reg.expand_mentions("@git-release");
        assert!(loaded == vec!["git-release"]);
        assert!(prompt.contains("git-release"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mencion_inexistente_se_reporta_y_no_carga() {
        let reg = Registry::default();
        let (prompt, ctx, loaded, missing) = reg.expand_mentions("hola @noexiste chau");
        assert_eq!(prompt, "hola chau");
        assert!(loaded.is_empty());
        assert!(ctx.is_empty());
        assert_eq!(missing, vec!["noexiste"]);
        // Un `@` dentro de una palabra (email) no se toma como mención.
        let (prompt, _, _, missing) = reg.expand_mentions("mandá a x@y.com");
        assert_eq!(prompt, "mandá a x@y.com");
        assert!(missing.is_empty());
    }

    #[test]
    fn mencion_de_un_skill_deny_no_se_precarga() {
        let root = temp_dir("mention-deny");
        write(
            &root.join("secreto/SKILL.md"),
            "---\ndescription: d\n---\nx",
        );
        let rules = vec![SkillRule::new("secreto", SkillEffect::Deny)];
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root.clone())])
            .with_permissions(&rules, true);
        let (_, ctx, loaded, missing) = reg.expand_mentions("@secreto");
        assert!(loaded.is_empty());
        assert!(ctx.is_empty());
        assert_eq!(missing, vec!["secreto"]);
        let _ = fs::remove_dir_all(&root);
    }
}
