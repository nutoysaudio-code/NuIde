//! CRUD de los proveedores del agente **NuCoder**.
//!
//! Edita la config propia del coder: `~/.config/nucoder/config.json`, la misma
//! que lee NuCoder:
//!
//! ```json
//! {
//!   "free_only": false,
//!   "default_model": null,
//!   "providers": [
//!     { "id": "openrouter", "name": "OpenRouter",
//!       "base_url": "https://openrouter.ai/api/v1", "api_key": "sk-…",
//!       "free_only": true, "models": [ { "id": "…:free", "free": true } ] }
//!   ]
//! }
//! ```
//!
//! Se conservan las claves desconocidas, tanto del archivo como de cada
//! proveedor (por si el coder agrega campos más adelante).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Un proveedor de NuCoder.
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub free_only: bool,
    /// Array de modelos como texto JSON (se valida al guardar).
    pub models: String,
    /// Claves del objeto original que el formulario no edita.
    extra: serde_json::Map<String, Value>,
}

const KNOWN: [&str; 6] = ["id", "name", "base_url", "api_key", "free_only", "models"];

impl Provider {
    /// Construye un proveedor desde el objeto JSON de la config.
    fn from_value(v: &Value) -> Option<Provider> {
        let obj = v.as_object()?;
        let id = obj.get("id").and_then(Value::as_str)?.to_string();
        let models = serde_json::to_string(obj.get("models").unwrap_or(&json!([])))
            .unwrap_or_else(|_| "[]".to_string());
        let mut extra = obj.clone();
        for k in KNOWN {
            extra.remove(k);
        }
        Some(Provider {
            id,
            name: obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            base_url: obj
                .get("base_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            api_key: obj
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            free_only: obj
                .get("free_only")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            models,
            extra,
        })
    }

    /// Crea un proveedor nuevo desde los campos del formulario. `models` ya debe
    /// estar validado como array JSON.
    pub fn from_fields(
        id: &str,
        name: &str,
        base_url: &str,
        api_key: &str,
        free_only: bool,
        models: &str,
    ) -> Provider {
        Provider {
            id: id.into(),
            name: name.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            free_only,
            models: models.into(),
            extra: serde_json::Map::new(),
        }
    }

    pub fn set_name(&mut self, v: &str) {
        self.name = v.into();
    }

    pub fn set_base_url(&mut self, v: &str) {
        self.base_url = v.into();
    }

    pub fn set_api_key(&mut self, v: &str) {
        self.api_key = v.into();
    }

    pub fn set_free_only(&mut self, v: bool) {
        self.free_only = v;
    }

    /// Reemplaza los modelos (`v` ya validado como array JSON).
    pub fn set_models(&mut self, v: &str) {
        self.models = v.into();
    }

    /// Cambia el id.
    pub fn rename(&mut self, new_id: &str) {
        self.id = new_id.into();
    }

    /// Arma el objeto JSON del proveedor para escribir la config.
    fn to_value(&self) -> Value {
        let mut obj = self.extra.clone();
        obj.insert("id".into(), json!(self.id));
        obj.insert("name".into(), json!(self.name));
        obj.insert("base_url".into(), json!(self.base_url));
        obj.insert("api_key".into(), json!(self.api_key));
        obj.insert("free_only".into(), json!(self.free_only));
        let models: Value = serde_json::from_str(&self.models).unwrap_or_else(|_| json!([]));
        obj.insert("models".into(), models);
        Value::Object(obj)
    }
}

/// Ruta de la config propia de NuCoder.
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("NUCODER_CONFIG") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("nucoder").join("config.json")
}

/// Lee los proveedores de la config propia.
pub fn load() -> Vec<Provider> {
    load_from(&config_path())
}

/// Lee los proveedores de un archivo `config.json` dado (para tests).
pub fn load_from(path: &Path) -> Vec<Provider> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    v.get("providers")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Provider::from_value).collect())
        .unwrap_or_default()
}

/// Escribe los proveedores en la config propia (conserva el resto del archivo).
pub fn save(providers: &[Provider]) -> Result<String, String> {
    save_to(&config_path(), providers)
}

/// Escribe los proveedores en un archivo `config.json` dado (para tests).
pub fn save_to(path: &Path, providers: &[Provider]) -> Result<String, String> {
    let mut doc: Value = fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let arr: Vec<Value> = providers.iter().map(Provider::to_value).collect();
    doc["providers"] = Value::Array(arr);
    let mut text =
        serde_json::to_string_pretty(&doc).map_err(|e| format!("no se pudo serializar: {e}"))?;
    text.push('\n');
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, text).map_err(|e| format!("no se pudo escribir {}: {e}", path.display()))?;
    Ok(format!(
        "{} proveedor(es) guardado(s) en {}",
        providers.len(),
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prefijos típicos de API keys.
    const KEY_PREFIXES: [&str; 5] = ["sk-", "gsk_", "nvapi-", "AIza", "xai-"];

    /// ¿El texto tiene algo con forma de API key? (prefijo + ≥16 alfanuméricos).
    fn file_tiene_key(text: &str) -> bool {
        for line in text.lines() {
            for p in KEY_PREFIXES {
                let mut from = 0;
                while let Some(pos) = line[from..].find(p) {
                    let start = from + pos + p.len();
                    let run = line[start..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .count();
                    if run >= 16 {
                        return true;
                    }
                    from = start;
                }
            }
        }
        false
    }

    /// Guardián: **ninguna API key debe quedar versionada** en el repo. Las keys
    /// viven en `~/.config/nucoder/config.json` (fuera del repo). Se saltea si no
    /// hay `git`.
    #[test]
    fn el_repo_no_tiene_keys_versionadas() {
        let root = env!("CARGO_MANIFEST_DIR");
        let out = crate::proc::hidden("git")
            .args(["ls-files"])
            .current_dir(root)
            .output();
        let Ok(out) = out else {
            eprintln!("test salteado: no se pudo correr git");
            return;
        };
        if !out.status.success() {
            eprintln!("test salteado: `git ls-files` falló");
            return;
        }
        let files = String::from_utf8_lossy(&out.stdout);
        let mut bad: Vec<String> = Vec::new();
        for f in files.lines() {
            let path = std::path::Path::new(root).join(f);
            let Ok(text) = fs::read_to_string(&path) else {
                continue; // binario o no legible
            };
            if file_tiene_key(&text) {
                bad.push(f.to_string());
            }
        }
        assert!(bad.is_empty(), "posibles API keys versionadas: {bad:?}");
    }

    #[test]
    fn el_detector_reconoce_keys_y_ignora_ejemplos() {
        // Se arman por partes para que ESTE archivo no parezca tener una key.
        let sk = format!("sk-{}", "or-v1-abcdefghijklmnop0123");
        let gsk = format!("gsk_{}", "abcdefghijklmnopq");
        assert!(file_tiene_key(&format!("api_key = \"{sk}\"")));
        assert!(file_tiene_key(&format!("Authorization: Bearer {gsk}")));
        // Ejemplos del README (con `...`) NO cuentan.
        assert!(!file_tiene_key("--api-key sk-or-..."));
        assert!(!file_tiene_key("--api-key sk-ant-..."));
        assert!(!file_tiene_key("sin nada parecido"));
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> TestDir {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!(
                "nucoder-providers-test-{}-{}",
                std::process::id(),
                n
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).expect("crear temp dir");
            TestDir(p)
        }

        fn config(&self) -> PathBuf {
            self.0.join("config.json")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn carga_y_edita_conservando_claves() {
        let dir = TestDir::new();
        fs::write(
            dir.config(),
            r#"{
  "free_only": true,
  "providers": [
    { "id": "openrouter", "name": "OpenRouter", "base_url": "https://openrouter.ai/api/v1",
      "api_key": "sk-1", "free_only": true, "cot": "extra",
      "models": [ { "id": "x:free", "free": true } ] }
  ]
}"#,
        )
        .unwrap();

        let mut list = load_from(&dir.config());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "openrouter");
        assert!(list[0].free_only);
        assert!(list[0].models.contains("x:free"));

        let p = &mut list[0];
        p.set_base_url("https://nuevo/v1");
        p.set_api_key("sk-9");
        p.rename("or2");
        // Cambiar el id no debe duplicar claves.
        assert!(!list.iter().any(|x| x.id == "openrouter"));
        save_to(&dir.config(), &list).unwrap();

        let text = fs::read_to_string(dir.config()).unwrap();
        assert!(text.contains("https://nuevo/v1"));
        assert!(text.contains("sk-9"));
        assert!(text.contains("\"or2\""));
        // La clave desconocida del proveedor se conserva.
        assert!(text.contains("\"cot\""));
        // El `free_only` de la raíz también.
        assert!(text.contains("\"free_only\": true"));

        let reloaded = load_from(&dir.config());
        assert_eq!(reloaded[0].id, "or2");
        assert_eq!(reloaded[0].api_key, "sk-9");
    }

    #[test]
    fn crea_proveedor_nuevo_desde_campos() {
        let dir = TestDir::new();
        let p = Provider::from_fields(
            "gemini",
            "Gemini",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "AIza",
            false,
            r#"[ { "id": "gemini-2.0", "free": false } ]"#,
        );
        save_to(&dir.config(), &[p]).unwrap();
        let list = load_from(&dir.config());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "gemini");
        assert!(list[0].models.contains("gemini-2.0"));
        assert!(!list[0].free_only);
    }
}
