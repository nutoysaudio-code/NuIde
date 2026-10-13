//! Configuración de NuCoder.
//!
//! Única fuente: la config propia del CLI, `~/.config/nucoder/config.json`
//! (o `$NUCODER_CONFIG`). Es donde el usuario carga las **API keys** de los
//! modelos gratuitos.
//!
//! El costo se normaliza a un entero (`cost_scaled`, ×100 USD/Mtok) y la
//! capacidad a `power` (0..9), para que la decisión del router viva en SpASM
//! comparando enteros.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::caps::{self, Cap};
use crate::client;
use crate::ffi;
use crate::freefilter::{self, FreeFilter};
use crate::mcp::McpServer;
use crate::skills::{SkillEffect, SkillRule};

/// Las tareas que entiende el router (prefijo `/tarea` en la TUI).
pub const TASKS: [&str; 6] = [
    "complete",
    "refactor",
    "debug",
    "explain",
    "test",
    "architect",
];

/// Un modelo de chat "curado" para la UI: sólo los ruteables (`text`), gratis
/// primero y ordenados por utilidad (código → visión → texto → resto, luego
/// potencia). Es lo que ofrece el selector del panel.
#[derive(Debug, Clone, PartialEq)]
pub struct CuratedModel {
    pub id: String,
    pub label: String,
    pub caps: Vec<Cap>,
    pub free: bool,
    pub power: u8,
}

/// Prioridad de capacidad para ordenar (lo más útil para el coder, primero).
fn cap_rank(caps: &[Cap]) -> usize {
    const ORDER: [Cap; 8] = [
        Cap::Code,
        Cap::Vision,
        Cap::Text,
        Cap::Audio,
        Cap::Image,
        Cap::Video,
        Cap::Embedding,
        Cap::Guard,
    ];
    caps.iter()
        .map(|c| ORDER.iter().position(|o| o == c).unwrap_or(9))
        .min()
        .unwrap_or(9)
}

/// Un modelo de un proveedor.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub free: bool,
    pub power: u8,
    pub cost_scaled: i64,
    /// Capacidades (texto, código, visión, imagen, audio, video…).
    pub caps: Vec<Cap>,
}

/// Un proveedor (endpoint OpenAI-compatible + su key).
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    /// Si es verdadero, sólo se ofrecen los modelos gratuitos de este proveedor.
    pub free_only: bool,
    /// Cómo decidir si un modelo es gratis al descubrir.
    pub free_filter: FreeFilter,
    /// Marcadores/globs propios (si vacío, los por defecto).
    pub free_match: Vec<String>,
    pub models: Vec<Model>,
}

/// Configuración cargada (unión de las dos fuentes).
#[derive(Debug, Clone)]
pub struct Config {
    pub providers: Vec<Provider>,
    /// Campo `model` global (referencia opcional, hoy sólo informativa).
    pub default_model: Option<String>,
    /// Si es verdadero, el router sólo considera modelos gratuitos.
    pub free_only: bool,
    /// Confina las tools de archivos del agente a la raíz del proyecto.
    pub sandbox: bool,
    /// Pide confirmación (TUI) antes de acciones sensibles.
    pub confirm: bool,
    /// **Auto-aprobar**: si es `true`, el agente no pide confirmación para
    /// acciones sensibles (borrar/mover/bash) y las ejecuta directamente. Pisa
    /// a `confirm`. Es el "switch" que se prende desde el panel (o por CLI).
    pub auto_approve: bool,
    /// --- permisos (pestaña Permisos del panel) ---
    /// Permitir `web_fetch`/`web_search` (navegar/consultar internet).
    pub perm_web: bool,
    /// Permitir **leer** fuera del proyecto (el sandbox sólo confina escrituras).
    pub perm_read_outside: bool,
    /// Permitir `run_in_terminal` (escribir en las terminales de la IDE).
    pub perm_terminal: bool,
    /// Permitir el cliente FTP (`ftp_*` sobre el cliente abierto en la IDE).
    pub perm_ftp: bool,
    /// **Autopiloto**: tras aprobar un plan, sigue hasta terminarlo sin pedir
    /// confirmación en cada paso.
    pub autopilot: bool,
    /// Rutas extra de la KB local (`read_docs`).
    pub knowledge: Vec<String>,
    /// Rutas extra de **skills** (además de las de Claude/OpenCode/agentes).
    pub skills: Vec<String>,
    /// Reglas de permiso por skill (`skills_permissions`): id (glob) → efecto
    /// (`allow`/`ask`/`deny`; la última que matchea gana).
    pub skills_permissions: Vec<SkillRule>,
    /// Si es `false`, no se anuncia ningún skill (igual se cargan por id).
    pub skills_autoinvoke: bool,
    /// Servidores **MCP** (Model Context Protocol) a exponer como herramientas.
    pub mcp: Vec<McpServer>,
    /// Marca el system como cacheable (prompt caching del proveedor).
    pub prompt_cache: bool,
    /// Tope aproximado de contexto (caracteres) para la compactación.
    pub context_chars: usize,
    /// Modelos que **no funcionan** (404/timeout/5xx/respuesta vacía): el router
    /// los saltea. Los marca el panel al rotar (evento `Dead`) o al hacer ping.
    pub model_dead: Vec<String>,
    /// Modelos que **respondieron** en el último ping (para filtrar «solo ok»).
    pub model_ok: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            providers: Vec::new(),
            default_model: None,
            // Gratis por defecto: el router sólo considera modelos gratuitos
            // (se puede apagar desde el panel).
            free_only: true,
            // El sandbox y la confirmación arrancan activos (más seguro).
            sandbox: true,
            confirm: true,
            auto_approve: false,
            // Permisos: internet y terminales disponibles; leer fuera del
            // proyecto, no; autopiloto, apagado.
            perm_web: true,
            perm_read_outside: false,
            perm_terminal: true,
            perm_ftp: true,
            autopilot: false,
            knowledge: Vec::new(),
            skills: Vec::new(),
            skills_permissions: Vec::new(),
            skills_autoinvoke: true,
            mcp: Vec::new(),
            prompt_cache: false,
            context_chars: 48_000,
            model_dead: Vec::new(),
            model_ok: Vec::new(),
        }
    }
}

/// Un (proveedor, modelo) listo para pedirle al router.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub provider_idx: usize,
    pub model_idx: usize,
    pub label: String,
    pub free: bool,
    pub power: u8,
    pub cost_scaled: i64,
    /// Índice dentro de la lista de candidatos (el que ve el router).
    pub flat: usize,
}

impl Config {
    /// Carga la config propia.
    pub fn load() -> Self {
        let mut cfg = Config::default();
        let mut seen: HashSet<String> = HashSet::new();

        if let Some(v) = read_json(&own_path()) {
            cfg.free_only = v.get("free_only").and_then(Value::as_bool).unwrap_or(true);
            cfg.sandbox = v.get("sandbox").and_then(Value::as_bool).unwrap_or(true);
            cfg.confirm = v.get("confirm").and_then(Value::as_bool).unwrap_or(true);
            cfg.auto_approve = v
                .get("auto_approve")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(p) = v.get("permissions") {
                cfg.perm_web = p.get("web").and_then(Value::as_bool).unwrap_or(true);
                cfg.perm_read_outside = p
                    .get("read_outside")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                cfg.perm_terminal = p.get("terminal").and_then(Value::as_bool).unwrap_or(true);
                cfg.perm_ftp = p.get("ftp").and_then(Value::as_bool).unwrap_or(true);
            }
            cfg.autopilot = v.get("autopilot").and_then(Value::as_bool).unwrap_or(false);
            cfg.prompt_cache = v
                .get("prompt_cache")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            cfg.context_chars = v
                .get("context_chars")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .filter(|n| *n >= 2_000)
                .unwrap_or(48_000);
            cfg.knowledge = v
                .get("knowledge")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            cfg.skills = v
                .get("skills")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            cfg.skills_permissions = v
                .get("skills_permissions")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(parse_skill_rule).collect())
                .unwrap_or_default();
            cfg.skills_autoinvoke = v
                .get("skills_autoinvoke")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            cfg.mcp = v
                .get("mcp")
                .and_then(|m| m.get("servers"))
                .and_then(Value::as_object)
                .map(|servers| {
                    servers
                        .iter()
                        .filter_map(|(id, sv)| McpServer::from_value(id, sv))
                        .collect()
                })
                .unwrap_or_default();
            cfg.model_dead = v
                .get("model_dead")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            cfg.model_ok = v
                .get("model_ok")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            cfg.default_model = v
                .get("default_model")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if let Some(list) = v.get("providers").and_then(Value::as_array) {
                for p in list {
                    if let Some(prov) = parse_own_provider(p) {
                        seen.insert(prov.id.clone());
                        cfg.providers.push(prov);
                    }
                }
            }
        }

        cfg
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Alta o reemplazo de un proveedor propio (por id).
    pub fn upsert_own(&mut self, prov: Provider) {
        if let Some(existing) = self.providers.iter_mut().find(|p| p.id == prov.id) {
            *existing = prov;
        } else {
            self.providers.push(prov);
        }
    }

    /// Elimina un proveedor. `true` si lo encontró.
    pub fn remove_own(&mut self, id: &str) -> bool {
        let before = self.providers.len();
        self.providers.retain(|p| p.id != id);
        self.providers.len() != before
    }

    /// Marca un modelo como **roto** (falló) y lo saca de la lista de «ok».
    pub fn mark_dead(&mut self, id: &str) {
        self.model_ok.retain(|m| m != id);
        if !self.model_dead.iter().any(|m| m == id) {
            self.model_dead.push(id.to_string());
        }
    }

    /// Marca un modelo como **funcional** (el ping respondió) y lo saca de «roto».
    pub fn mark_ok(&mut self, id: &str) {
        self.model_dead.retain(|m| m != id);
        if !self.model_ok.iter().any(|m| m == id) {
            self.model_ok.push(id.to_string());
        }
    }

    /// Borra el estado aprendido (muertos/ok) para volver a empezar.
    pub fn clear_status(&mut self) {
        self.model_dead.clear();
        self.model_ok.clear();
    }

    /// Escribe la config propia.
    /// ¿El agente debe **pedir confirmación** antes de una acción sensible?
    /// `auto_approve` gana: si está prendido, nunca se pregunta.
    pub fn asks_confirmation(&self) -> bool {
        self.confirm && !self.auto_approve
    }

    pub fn save_own(&self) -> std::io::Result<PathBuf> {
        let path = own_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let providers: Vec<Value> = self.providers.iter().map(provider_to_json).collect();
        let doc = json!({
            "default_model": self.default_model,
            "free_only": self.free_only,
            "sandbox": self.sandbox,
            "confirm": self.confirm,
            "auto_approve": self.auto_approve,
            "permissions": {
                "web": self.perm_web,
                "read_outside": self.perm_read_outside,
                "terminal": self.perm_terminal,
                "ftp": self.perm_ftp,
            },
            "autopilot": self.autopilot,
            "knowledge": self.knowledge,
            "skills": self.skills,
            "skills_autoinvoke": self.skills_autoinvoke,
            "skills_permissions": self
                .skills_permissions
                .iter()
                .map(|r| json!({ "id": r.id, "effect": r.effect.as_str() }))
                .collect::<Vec<_>>(),
            "mcp": {
                "servers": Value::Object(
                    self.mcp
                        .iter()
                        .map(|s| (s.id.clone(), s.to_value()))
                        .collect(),
                ),
            },
            "prompt_cache": self.prompt_cache,
            "context_chars": self.context_chars,
            "model_dead": self.model_dead,
            "model_ok": self.model_ok,
            "providers": providers,
        });
        let mut text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        fs::write(&path, text)?;
        Ok(path)
    }

    /// Aplana proveedores/modelos a candidatos. `free_only` (además del flag por
    /// proveedor) deja pasar sólo los modelos gratuitos; `cap` filtra por
    /// capacidad (texto/código/visión/imagen/audio/video…).
    pub fn candidates(&self, free_only: bool, cap: Option<Cap>, vision: bool) -> Vec<Candidate> {
        // Los modelos marcados como «no funciona» no entran al ruteo.
        let dead: HashSet<&str> = self.model_dead.iter().map(String::as_str).collect();
        let mut out = Vec::new();
        for (pi, p) in self.providers.iter().enumerate() {
            if p.base_url.trim().is_empty() {
                continue;
            }
            let only_free = free_only || p.free_only;
            for (mi, m) in p.models.iter().enumerate() {
                if dead.contains(m.id.as_str()) {
                    continue;
                }
                if only_free && !m.free {
                    continue;
                }
                if let Some(c) = cap {
                    if !m.caps.contains(&c) {
                        continue;
                    }
                }
                if vision && !m.caps.contains(&Cap::Vision) {
                    continue;
                }
                let flat = out.len();
                out.push(Candidate {
                    provider_idx: pi,
                    model_idx: mi,
                    label: format!("{}/{}", p.name, m.name),
                    free: m.free,
                    power: m.power,
                    cost_scaled: m.cost_scaled,
                    flat,
                });
            }
        }
        out
    }

    /// Busca un modelo por id (exacto, luego sufijo, luego "contiene"),
    /// ignorando los filtros de gratis/capacidad: es una elección explícita.
    pub fn find_candidate(&self, id: &str) -> Option<Candidate> {
        let q = id.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        let mut best: Option<(u8, Candidate)> = None;
        for (pi, p) in self.providers.iter().enumerate() {
            if p.base_url.trim().is_empty() {
                continue;
            }
            for (mi, m) in p.models.iter().enumerate() {
                let mid = m.id.to_lowercase();
                let score = if mid == q {
                    3
                } else if mid.ends_with(&q) {
                    2
                } else if mid.contains(&q) {
                    1
                } else {
                    0
                };
                if score == 0 || best.as_ref().is_some_and(|(s, _)| score <= *s) {
                    continue;
                }
                best = Some((
                    score,
                    Candidate {
                        provider_idx: pi,
                        model_idx: mi,
                        label: format!("{}/{}", p.name, m.name),
                        free: m.free,
                        power: m.power,
                        cost_scaled: m.cost_scaled,
                        flat: 0,
                    },
                ));
            }
        }
        best.map(|(_, c)| c)
    }

    /// Modelos de chat curados para la UI: sólo los ruteables (`text`), gratis
    /// primero, ordenados por capacidad y potencia.
    pub fn curated_models(&self) -> Vec<CuratedModel> {
        let mut out: Vec<CuratedModel> = Vec::new();
        for p in &self.providers {
            for m in &p.models {
                if !m.caps.contains(&Cap::Text) {
                    continue;
                }
                out.push(CuratedModel {
                    id: m.id.clone(),
                    label: format!("{}/{}", p.id, m.name),
                    caps: m.caps.clone(),
                    free: m.free,
                    power: m.power,
                });
            }
        }
        out.sort_by(|a, b| {
            (!a.free)
                .cmp(&(!b.free))
                .then(cap_rank(&a.caps).cmp(&cap_rank(&b.caps)))
                .then(b.power.cmp(&a.power))
                .then(a.id.cmp(&b.id))
        });
        out
    }

    /// Decide el candidato para `task` con el router SpASM, saltando `excluded`.
    pub fn route(
        &self,
        task: &str,
        excluded: &[usize],
        free_only: bool,
        cap: Option<Cap>,
        vision: bool,
    ) -> Option<Candidate> {
        let cands = self.candidates(free_only, cap, vision);
        if cands.is_empty() {
            return None;
        }
        let spec = cands
            .iter()
            .map(|c| format!("{},{},{}", c.free as u8, c.power, c.cost_scaled))
            .collect::<Vec<_>>()
            .join(";");
        let idx = ffi::router_pick(task, &spec, excluded)?;
        cands.into_iter().nth(idx)
    }
}

// ---------------------------------------------------------------------------
// Parseo de la config propia
// ---------------------------------------------------------------------------

/// Construye un proveedor propio desde un JSON (lo usa la CLI para dar de alta
/// proveedores: `provider add`). Misma lógica que la config en disco.
pub fn own_provider_from_value(v: &Value) -> Option<Provider> {
    parse_own_provider(v)
}

/// Parsea una regla de permiso de skill (`{"id": glob, "effect": "..."}`).
fn parse_skill_rule(v: &Value) -> Option<SkillRule> {
    let id = v.get("id").and_then(Value::as_str)?.trim();
    if id.is_empty() {
        return None;
    }
    let effect = v
        .get("effect")
        .or_else(|| v.get("permission"))
        .and_then(Value::as_str)
        .map(SkillEffect::parse)
        .unwrap_or(SkillEffect::Allow);
    Some(SkillRule::new(id, effect))
}

fn parse_own_provider(v: &Value) -> Option<Provider> {
    let id = v.get("id").and_then(Value::as_str)?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let name = str_key(v, &["name"]).unwrap_or_else(|| id.clone());
    let base_url = str_key(v, &["base_url", "baseURL", "baseUrl"])
        .or_else(|| str_field(v, &["options", "baseURL"]))
        .or_else(|| str_field(v, &["settings", "baseURL"]))
        .unwrap_or_default();
    let api_key = str_key(v, &["api_key", "apiKey"])
        .or_else(|| str_field(v, &["options", "apiKey"]))
        .or_else(|| str_field(v, &["settings", "apiKey"]))
        .or_else(|| env_api_key(v))
        .unwrap_or_default();
    let free_only = v.get("free_only").and_then(Value::as_bool).unwrap_or(false)
        || v.get("freeOnly").and_then(Value::as_bool).unwrap_or(false);
    let free_match: Vec<String> = v
        .get("free_match")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // Filtro explícito o, si falta, el preset del proveedor (o `Auto`).
    let free_filter = v
        .get("free_filter")
        .and_then(Value::as_str)
        .and_then(FreeFilter::parse)
        .unwrap_or_else(|| freefilter::default_filter(&id));

    let mut models = Vec::new();
    if let Some(arr) = v.get("models").and_then(Value::as_array) {
        for m in arr {
            models.push(parse_own_model(m));
        }
    }

    Some(Provider {
        id,
        name,
        base_url,
        api_key,
        free_only,
        free_filter,
        free_match,
        models,
    })
}

fn parse_own_model(v: &Value) -> Model {
    if let Some(id) = v.as_str() {
        let id = id.to_string();
        let free = looks_free(&id, &id);
        let caps = caps::infer(&id, &id, &Value::Null);
        return Model {
            power: guess_power(&id),
            name: id.clone(),
            free,
            cost_scaled: 0,
            caps,
            id,
        };
    }
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| v.get("model").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let name = str_field(v, &["name"]).unwrap_or_else(|| id.clone());
    let cost_scaled = own_cost(v);
    let free = v
        .get("free")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| looks_free(&id, &name) || cost_scaled == 0);
    let power = v
        .get("power")
        .and_then(Value::as_u64)
        .map(|p| p.min(9) as u8)
        .unwrap_or_else(|| guess_power(&format!("{id} {name}")));
    // Capacidades: declaradas o inferidas del crudo/nombre.
    let caps = caps::explicit(v).unwrap_or_else(|| caps::infer(&id, &name, v));
    Model {
        id,
        name,
        free,
        power,
        cost_scaled,
        caps,
    }
}

fn own_cost(v: &Value) -> i64 {
    match v.get("cost") {
        Some(Value::Number(n)) => (n.as_f64().unwrap_or(0.0) * 100.0).round() as i64,
        Some(Value::Object(_)) => cost_from_object(v.get("cost").unwrap()),
        _ => 0,
    }
}

fn cost_from_object(c: &Value) -> i64 {
    let get = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    ((get("input") + get("output")) * 100.0).round() as i64
}

// ---------------------------------------------------------------------------
// Heurísticas y helpers
// ---------------------------------------------------------------------------

/// Busca el primer string en una ruta anidada (`&["options","baseURL"]`).
fn str_field(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_str().map(str::to_string).filter(|s| !s.is_empty())
}

/// Primer campo presente entre varias claves alternativas (`base_url`/`baseURL`).
fn str_key(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Primera variable de entorno presente en `env` (array).
fn env_api_key(v: &Value) -> Option<String> {
    let list = v.get("env")?.as_array()?;
    for item in list {
        if let Some(name) = item.as_str() {
            if let Ok(val) = std::env::var(name) {
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// ¿El modelo se anuncia como gratuito por su nombre/id? (sufijo `:free`, o la
/// palabra "free"/"gratis"/"trial"). Es sólo una pista: hay endpoints (p. ej.
/// NVIDIA NIM) en los que el catálogo **es** la lista de endpoints libres y no
/// etiquetan nada.
pub fn looks_free(id: &str, name: &str) -> bool {
    let l = format!("{id} {name}").to_lowercase();
    l.contains(":free") || l.contains("free") || l.contains("gratis") || l.contains("trial")
}

/// Capacidad aproximada 0..9 a partir del nombre (proxy del tamaño/calidad).
/// Un modelo recién descubierto en el endpoint, listo para fusionar.
#[derive(Debug, Clone)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: String,
    /// Si el filtro del proveedor lo marca como gratis.
    pub free: bool,
    /// Capacidades inferidas.
    pub caps: Vec<Cap>,
}

/// Fusiona el catálogo **descubierto** con los modelos guardados: conserva el
/// `free` fijado a mano (salvo `reset_free`) y el costo, y recalcula
/// nombre/potencia/capacidades si el descubrimiento los trae.
pub fn merge_discovered(
    existing: &[Model],
    discovered: &[DiscoveredModel],
    reset_free: bool,
) -> Vec<Model> {
    discovered
        .iter()
        .map(|d| {
            let prev = existing.iter().find(|m| m.id == d.id);
            let free = if reset_free {
                d.free
            } else {
                prev.map(|p| p.free).unwrap_or(d.free)
            };
            let name = if d.name.trim().is_empty() {
                prev.map(|p| p.name.clone()).unwrap_or_else(|| d.id.clone())
            } else {
                d.name.clone()
            };
            let caps = if d.caps.is_empty() {
                prev.map(|p| p.caps.clone()).unwrap_or_default()
            } else {
                d.caps.clone()
            };
            Model {
                id: d.id.clone(),
                name,
                free,
                power: prev.map(|p| p.power).unwrap_or_else(|| guess_power(&d.id)),
                cost_scaled: prev.map(|p| p.cost_scaled).unwrap_or(0),
                caps,
            }
        })
        .collect()
}

/// Re-baja el catálogo de cada proveedor (o del indicado) y lo **fusiona** con
/// los modelos guardados (conserva los `free` a mano salvo `reset_free`).
/// Devuelve, por proveedor, el número de modelos o el error.
pub fn refresh_catalog(
    cfg: &mut Config,
    only: Option<&str>,
    reset_free: bool,
) -> Vec<(String, Result<usize, String>)> {
    let ids: Vec<String> = match only {
        Some(id) => vec![id.to_string()],
        None => cfg.providers.iter().map(|p| p.id.clone()).collect(),
    };
    let mut out = Vec::new();
    for id in ids {
        let Some(prov) = cfg.provider(&id).cloned() else {
            out.push((id, Err("no existe".into())));
            continue;
        };
        match client::list_models(&prov.base_url, &prov.api_key) {
            Ok(models) if !models.is_empty() => {
                let discovered: Vec<DiscoveredModel> = models
                    .iter()
                    .map(|m| DiscoveredModel {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        free: freefilter::classify(
                            prov.free_filter,
                            &m.raw,
                            &m.id,
                            &m.name,
                            &prov.free_match,
                            prov.free_only,
                        ),
                        caps: caps::infer(&m.id, &m.name, &m.raw),
                    })
                    .collect();
                let merged = merge_discovered(&prov.models, &discovered, reset_free);
                let n = merged.len();
                let mut newp = prov;
                newp.models = merged;
                cfg.upsert_own(newp);
                out.push((id, Ok(n)));
            }
            Ok(_) => out.push((id, Err("sin modelos".into()))),
            Err(e) => out.push((id, Err(e.to_string()))),
        }
    }
    out
}

fn guess_power(name: &str) -> u8 {
    let l = name.to_lowercase();
    let mut p: i32 = 3;
    for kw in [
        "max", "opus", "ultra", "405b", "235b", "sonnet", "reason", "thinking", "o1", "o3", "r1",
        "v3", "pro",
    ] {
        if l.contains(kw) {
            p += 2;
        }
    }
    for kw in ["70b", "72b", "coder", "plus", "large", "32b", "turbo"] {
        if l.contains(kw) {
            p += 1;
        }
    }
    for kw in [
        "mini", "small", "lite", "nano", "7b", "8b", "haiku", "flash", "3b", "1b",
    ] {
        if l.contains(kw) {
            p -= 1;
        }
    }
    p.clamp(0, 9) as u8
}

fn provider_to_json(p: &Provider) -> Value {
    let models: Vec<Value> = p
        .models
        .iter()
        .map(|m| {
            let caps: Vec<&str> = m.caps.iter().map(|c| c.as_str()).collect();
            json!({
                "id": m.id,
                "name": m.name,
                "free": m.free,
                "power": m.power,
                "caps": caps,
            })
        })
        .collect();
    json!({
        "id": p.id,
        "name": p.name,
        "base_url": p.base_url,
        "api_key": p.api_key,
        "free_only": p.free_only,
        "free_filter": p.free_filter.as_str(),
        "free_match": p.free_match,
        "models": models,
    })
}

/// Carpeta de config propia del CLI.
fn own_path() -> PathBuf {
    if let Some(p) = std::env::var_os("NUCODER_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("nucoder")
        .join("config.json")
}

/// Ruta de la config propia (para mostrar en mensajes).
pub fn own_config_path() -> PathBuf {
    own_path()
}

/// Carpeta de la config propia (donde vive `config.json`).
pub fn config_dir() -> PathBuf {
    own_path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Lee y parsea un JSON (tolerando comentarios `//` y `/* */` de `.jsonc`).
fn read_json(path: &std::path::Path) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    // Tolerar el BOM UTF-8 (editores como Notepad lo agregan).
    let raw = raw.trim_start_matches('\u{feff}');
    parse_jsonc(raw)
}

fn parse_jsonc(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    serde_json::from_str::<Value>(&strip_comments(text)).ok()
}

/// Quita comentarios fuera de strings.
fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_from(json_text: &str) -> Config {
        let v: Value = serde_json::from_str(json_text).unwrap();
        let mut cfg = Config::default();
        let mut seen = HashSet::new();
        if let Some(list) = v.get("providers").and_then(Value::as_array) {
            for p in list {
                if let Some(prov) = parse_own_provider(p) {
                    seen.insert(prov.id.clone());
                    cfg.providers.push(prov);
                }
            }
        }
        cfg
    }

    #[test]
    fn curados_filtra_por_texto_y_ordena_free() {
        let mut cfg = Config::default();
        cfg.providers.push(Provider {
            id: "p".into(),
            name: "P".into(),
            base_url: "https://x/v1".into(),
            api_key: String::new(),
            free_only: false,
            free_filter: FreeFilter::Auto,
            free_match: Vec::new(),
            models: vec![
                Model {
                    id: "paga".into(),
                    name: "Paga".into(),
                    free: false,
                    power: 9,
                    cost_scaled: 100,
                    caps: vec![Cap::Text, Cap::Code],
                },
                Model {
                    id: "gratis".into(),
                    name: "Gratis".into(),
                    free: true,
                    power: 1,
                    cost_scaled: 0,
                    caps: vec![Cap::Text],
                },
                Model {
                    id: "solo-imagen".into(),
                    name: "Imagen".into(),
                    free: true,
                    power: 9,
                    cost_scaled: 0,
                    caps: vec![Cap::Image],
                },
            ],
        });
        let curados = cfg.curated_models();
        assert_eq!(curados.len(), 2, "sólo los de `text`");
        assert_eq!(curados[0].id, "gratis", "gratis primero");
        assert!(curados.iter().all(|m| m.caps.contains(&Cap::Text)));
    }

    #[test]
    fn merge_conserva_free_manual_salvo_reset() {
        let existing = vec![Model {
            id: "a".into(),
            name: "A".into(),
            free: true,
            power: 5,
            cost_scaled: 0,
            caps: vec![Cap::Text],
        }];
        let discovered = vec![
            DiscoveredModel {
                id: "a".into(),
                name: "A2".into(),
                free: false,
                caps: vec![Cap::Text, Cap::Code],
            },
            DiscoveredModel {
                id: "b".into(),
                name: "B".into(),
                free: true,
                caps: vec![Cap::Text],
            },
        ];
        let merged = merge_discovered(&existing, &discovered, false);
        assert_eq!(merged.len(), 2);
        let a = merged.iter().find(|m| m.id == "a").unwrap();
        assert!(a.free, "conserva el `free` manual");
        assert_eq!(a.name, "A2");
        assert!(a.caps.contains(&Cap::Code));
        assert!(merged.iter().find(|m| m.id == "b").unwrap().free);
        // `--reset-free`: se recalcula desde el filtro.
        let reset = merge_discovered(&existing, &discovered, true);
        assert!(!reset.iter().find(|m| m.id == "a").unwrap().free);
    }

    #[test]
    fn auto_approve_pisa_a_confirm() {
        let mut cfg = Config::default();
        assert!(cfg.asks_confirmation()); // confirm=true, auto=false
        cfg.auto_approve = true;
        assert!(!cfg.asks_confirmation()); // auto pisa
        cfg.auto_approve = false;
        cfg.confirm = false;
        assert!(!cfg.asks_confirmation());
    }

    #[test]
    fn strip_comments_no_rompe_urls() {
        let t = r#"{ "url": "https://x/v1", // nota
                       "b": 1 }"#;
        let s = strip_comments(t);
        assert!(s.contains("https://x/v1"));
        assert!(!s.contains("nota"));
    }

    #[test]
    fn guess_power_ordena() {
        assert!(guess_power("gpt-5.4-mini") < guess_power("gpt-5.4-max"));
        assert!(guess_power("deepseek-r1:free") >= guess_power("qwen-1b"));
    }

    #[test]
    fn looks_free_detecta_sufijo() {
        assert!(looks_free("deepseek/deepseek-r1:free", "R1"));
        assert!(!looks_free("gpt-5.4", "gpt-5.4"));
    }

    #[test]
    fn router_prefiere_gratis_y_rota() {
        // free=0/1, power, cost. El gratis (idx 2) gana; al excluirlo, va al
        // pago mas potente (idx 0).
        let cfg = cfg_from(
            r#"{
              "providers": [
                { "id": "paid", "base_url": "https://paid/v1", "models": [
                    { "id": "big-max", "free": false, "power": 9, "cost": 5.0 },
                    { "id": "small-mini", "free": false, "power": 2, "cost": 0.1 }
                ]},
                { "id": "free", "base_url": "https://free/v1", "models": [
                    { "id": "tiny:free", "free": true, "power": 1 }
                ]}
              ]
            }"#,
        );
        let c = cfg.route("refactor", &[], false, None, false).unwrap();
        assert_eq!(c.label, "free/tiny:free");
        // Con el gratis agotado, elige el pago mas potente.
        let c2 = cfg
            .route("refactor", &[c.flat], false, None, false)
            .unwrap();
        assert_eq!(c2.label, "paid/big-max");
        // free_only deja solo el gratuito.
        let cands = cfg.candidates(true, None, false);
        assert_eq!(cands.len(), 1);
        assert!(cands[0].free);
    }

    #[test]
    fn find_candidate_por_id() {
        let cfg = cfg_from(
            r#"{ "providers": [ { "id": "or", "base_url": "https://x/v1", "models": [
                { "id": "google/gemma:free", "name": "Gemma", "free": true },
                { "id": "meta/llama:free", "name": "Llama", "free": true }
            ]} ] }"#,
        );
        assert_eq!(
            cfg.find_candidate("google/gemma:free").unwrap().label,
            "or/Gemma"
        );
        // Sufijo.
        assert_eq!(cfg.find_candidate("llama:free").unwrap().label, "or/Llama");
        assert!(cfg.find_candidate("nope").is_none());
    }

    #[test]
    fn tarea_rapida_elige_barato_entre_gratis() {
        let cfg = cfg_from(
            r#"{
              "providers": [
                { "id": "f", "base_url": "https://f/v1", "models": [
                    { "id": "a:free", "free": true, "power": 5 },
                    { "id": "b:free", "free": true, "power": 2, "cost": 0.0 },
                    { "id": "c-paid-max", "free": false, "power": 9, "cost": 9.0 }
                ]}
              ]
            }"#,
        );
        // complete = rapida: entre gratuitos (costo 0) gana el de mayor power.
        let c = cfg.route("complete", &[], false, None, false).unwrap();
        assert!(c.free);
        assert_eq!(c.label, "f/a:free");
    }

    fn cfg_con_modelos(ids: &[(&str, bool)]) -> Config {
        let mut cfg = Config::default();
        cfg.providers.push(Provider {
            id: "p".into(),
            name: "P".into(),
            base_url: "https://x/v1".into(),
            api_key: String::new(),
            free_only: false,
            free_filter: FreeFilter::Auto,
            free_match: Vec::new(),
            models: ids
                .iter()
                .map(|(id, free)| Model {
                    id: (*id).into(),
                    name: (*id).into(),
                    free: *free,
                    power: 3,
                    cost_scaled: 0,
                    caps: vec![Cap::Text],
                })
                .collect(),
        });
        cfg
    }

    #[test]
    fn candidates_saltea_los_modelos_muertos() {
        let mut cfg = cfg_con_modelos(&[("a:free", true), ("b:free", true)]);
        assert_eq!(cfg.candidates(true, None, false).len(), 2);
        cfg.mark_dead("a:free");
        let cands = cfg.candidates(true, None, false);
        assert_eq!(cands.len(), 1, "el modelo muerto sale del ruteo");
        assert_eq!(cands[0].label, "P/b:free");
    }

    #[test]
    fn mark_dead_y_mark_ok_se_excluyen() {
        let mut cfg = Config::default();
        cfg.mark_ok("m");
        assert!(cfg.model_ok.contains(&"m".to_string()));
        cfg.mark_dead("m");
        assert!(cfg.model_dead.contains(&"m".to_string()));
        assert!(!cfg.model_ok.contains(&"m".to_string()));
        cfg.mark_ok("m");
        assert!(!cfg.model_dead.contains(&"m".to_string()));
        cfg.clear_status();
        assert!(cfg.model_dead.is_empty() && cfg.model_ok.is_empty());
    }

    #[test]
    fn gratis_por_defecto() {
        assert!(Config::default().free_only);
    }
}
