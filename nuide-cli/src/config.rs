//! Configuración de Nuide-CLI.
//!
//! Dos fuentes, unificadas (la propia gana):
//!
//!   1. **Propia** del CLI: `~/.config/nuide-cli/config.json` (o `$NUIDE_CLI_CONFIG`).
//!      Es donde el usuario carga las **API keys** de los modelos gratuitos.
//!   2. **De OpenCode**: `~/.config/opencode/opencode.json(c)` — se lee como
//!      respaldo, tratando los esquemas `provider` (singular) y `providers`
//!      (plural) como una unión.
//!
//! El costo se normaliza a un entero (`cost_scaled`, ×100 USD/Mtok) y la
//! capacidad a `power` (0..9), para que la decisión del router viva en SpASM
//! comparando enteros.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::ffi;

/// Las tareas que entiende el router (prefijo `/tarea` en la TUI).
pub const TASKS: [&str; 6] = [
    "complete",
    "refactor",
    "debug",
    "explain",
    "test",
    "architect",
];

/// De dónde salió un proveedor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Config propia del CLI (editable y persistente).
    Own,
    /// Config de OpenCode (sólo lectura).
    OpenCode,
}

/// Un modelo de un proveedor.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub free: bool,
    pub power: u8,
    pub cost_scaled: i64,
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
    pub source: Source,
    pub models: Vec<Model>,
}

/// Configuración cargada (unión de las dos fuentes).
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub providers: Vec<Provider>,
    /// Campo `model` global (referencia opcional, hoy sólo informativa).
    pub default_model: Option<String>,
    /// Si es verdadero, el router sólo considera modelos gratuitos.
    pub free_only: bool,
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
    /// Carga y unifica las dos fuentes.
    pub fn load() -> Self {
        let mut cfg = Config::default();
        let mut seen: HashSet<String> = HashSet::new();

        // 1. Config propia.
        if let Some(v) = read_json(&own_path()) {
            cfg.free_only = v.get("free_only").and_then(Value::as_bool).unwrap_or(false);
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

        // 2. Config de OpenCode (respaldo).
        if let Some(dir) = opencode_dir() {
            let mut disabled: HashSet<String> = HashSet::new();
            for file in ["opencode.json", "opencode.jsonc"] {
                let Some(text) = read_json(&dir.join(file)) else {
                    continue;
                };
                if cfg.default_model.is_none() {
                    cfg.default_model = text
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                if let Some(list) = text.get("disabled_providers").and_then(Value::as_array) {
                    for v in list {
                        if let Some(s) = v.as_str() {
                            disabled.insert(s.to_string());
                        }
                    }
                }
                for key in ["provider", "providers"] {
                    let Some(obj) = text.get(key).and_then(Value::as_object) else {
                        continue;
                    };
                    for (id, prov) in obj {
                        if disabled.contains(id) || seen.contains(id) {
                            continue;
                        }
                        if let Some(p) = parse_opencode_provider(id, prov) {
                            seen.insert(id.clone());
                            cfg.providers.push(p);
                        }
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

    /// Elimina un proveedor propio. `true` si lo encontró.
    pub fn remove_own(&mut self, id: &str) -> bool {
        let before = self.providers.len();
        self.providers
            .retain(|p| !(p.id == id && p.source == Source::Own));
        self.providers.len() != before
    }

    /// Escribe la config propia (sólo los proveedores `Own`).
    pub fn save_own(&self) -> std::io::Result<PathBuf> {
        let path = own_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let providers: Vec<Value> = self
            .providers
            .iter()
            .filter(|p| p.source == Source::Own)
            .map(provider_to_json)
            .collect();
        let doc = json!({
            "default_model": self.default_model,
            "free_only": self.free_only,
            "providers": providers,
        });
        let mut text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        fs::write(&path, text)?;
        Ok(path)
    }

    /// Aplana proveedores/modelos a candidatos. `free_only` (además del flag por
    /// proveedor) deja pasar sólo los modelos gratuitos.
    pub fn candidates(&self, free_only: bool) -> Vec<Candidate> {
        let mut out = Vec::new();
        for (pi, p) in self.providers.iter().enumerate() {
            if p.base_url.trim().is_empty() {
                continue;
            }
            let only_free = free_only || p.free_only;
            for (mi, m) in p.models.iter().enumerate() {
                if only_free && !m.free {
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

    /// Decide el candidato para `task` con el router SpASM, saltando `excluded`.
    pub fn route(&self, task: &str, excluded: &[usize], free_only: bool) -> Option<Candidate> {
        let cands = self.candidates(free_only);
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
        source: Source::Own,
        models,
    })
}

fn parse_own_model(v: &Value) -> Model {
    if let Some(id) = v.as_str() {
        let id = id.to_string();
        let free = looks_free(&id, &id);
        return Model {
            power: guess_power(&id),
            name: id.clone(),
            free,
            cost_scaled: 0,
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
    Model {
        id,
        name,
        free,
        power,
        cost_scaled,
    }
}

fn own_cost(v: &Value) -> i64 {
    match v.get("cost") {
        Some(Value::Number(n)) => (n.as_f64().unwrap_or(0.0) * 100.0).round() as i64,
        Some(Value::Object(_)) => cost_from_object(v.get("cost").unwrap()),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Parseo de la config de OpenCode
// ---------------------------------------------------------------------------

fn parse_opencode_provider(id: &str, prov: &Value) -> Option<Provider> {
    let base_url = str_field(prov, &["options", "baseURL"])
        .or_else(|| str_field(prov, &["settings", "baseURL"]))
        .unwrap_or_default();
    let api_key = str_field(prov, &["options", "apiKey"])
        .or_else(|| str_field(prov, &["settings", "apiKey"]))
        .or_else(|| env_api_key(prov))
        .unwrap_or_default();
    let name = str_field(prov, &["name"]).unwrap_or_else(|| id.to_string());

    let mut models = Vec::new();
    if let Some(obj) = prov.get("models").and_then(Value::as_object) {
        for (model_id, m) in obj {
            if m.get("disabled").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let mname = m
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(model_id)
                .to_string();
            let cost = m.get("cost").map(cost_from_object).unwrap_or(0);
            let free = looks_free(model_id, &mname) || cost == 0;
            models.push(Model {
                id: model_id.clone(),
                name: mname.clone(),
                free,
                power: guess_power(&format!("{model_id} {mname}")),
                cost_scaled: cost,
            });
        }
    }
    if models.is_empty() && base_url.is_empty() {
        return None;
    }
    Some(Provider {
        id: id.to_string(),
        name,
        base_url,
        api_key,
        free_only: false,
        source: Source::OpenCode,
        models,
    })
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

/// ¿El modelo se anuncia como gratuito? (sufijo `:free`, o la palabra "free").
fn looks_free(id: &str, name: &str) -> bool {
    let l = format!("{id} {name}").to_lowercase();
    l.contains(":free") || l.contains("free") || l.contains("gratis") || l.contains("trial")
}

/// Capacidad aproximada 0..9 a partir del nombre (proxy del tamaño/calidad).
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
            json!({
                "id": m.id,
                "name": m.name,
                "free": m.free,
                "power": m.power,
            })
        })
        .collect();
    json!({
        "id": p.id,
        "name": p.name,
        "base_url": p.base_url,
        "api_key": p.api_key,
        "free_only": p.free_only,
        "models": models,
    })
}

/// Carpeta de config propia del CLI.
fn own_path() -> PathBuf {
    if let Some(p) = std::env::var_os("NUIDE_CLI_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("nuide-cli")
        .join("config.json")
}

/// Ruta de la config propia (para mostrar en mensajes).
pub fn own_config_path() -> PathBuf {
    own_path()
}

/// Carpeta de config global de OpenCode: `~/.config/opencode`.
fn opencode_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    Some(home.join(".config").join("opencode"))
}

/// Lee y parsea un JSON (tolerando comentarios `//` y `/* */` de `.jsonc`).
fn read_json(path: &std::path::Path) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    parse_jsonc(&raw)
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
        let c = cfg.route("refactor", &[], false).unwrap();
        assert_eq!(c.label, "free/tiny:free");
        // Con el gratis agotado, elige el pago mas potente.
        let c2 = cfg.route("refactor", &[c.flat], false).unwrap();
        assert_eq!(c2.label, "paid/big-max");
        // free_only deja solo el gratuito.
        let cands = cfg.candidates(true);
        assert_eq!(cands.len(), 1);
        assert!(cands[0].free);
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
        let c = cfg.route("complete", &[], false).unwrap();
        assert!(c.free);
        assert_eq!(c.label, "f/a:free");
    }
}
