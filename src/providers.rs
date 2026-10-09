//! CRUD de los proveedores de OpenCode sobre la config global
//! (`~/.config/opencode/opencode.json(c)`).
//!
//! Edita **quirúrgicamente** el objeto `providers`: localiza el valor de la
//! clave y lo reemplaza (o lo inserta si falta); todo lo demás del archivo
//! queda byte a byte igual. Los archivos se tratan como una unión (el primero
//! gana si el mismo id aparece en varios) y al guardar los proveedores quedan
//! en `opencode.json` —que se crea si hace falta— y se sacan de
//! `opencode.jsonc` para no dejar copias viejas.

use std::fs;
use std::path::{Path, PathBuf};

use crate::jsonx;

/// Un proveedor de OpenCode. `raw` es el texto JSON del objeto (con llaves)
/// que se conserva entre versiones para no perder campos desconocidos
/// (`env`, `headers`, `modelID`, `cost`, `limit`, comentarios…).
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: String,
    /// Texto JSON crudo del objeto proveedor.
    pub raw: String,
    pub name: String,
    pub package: String,
    pub base_url: String,
    pub api_key: String,
    /// Texto JSON del objeto `models` ("{}" si el proveedor no trae).
    pub models: String,
}

/// Valor string dentro del objeto `settings` de un proveedor.
fn settings_string(raw: &str, key: &str) -> String {
    jsonx::find_key_value(raw, "settings", 0)
        .and_then(|loc| {
            let inner = &raw[loc.val_start..loc.val_end];
            jsonx::find_key_value(inner, key, 0)
                .and_then(|loc2| jsonx::string_value(inner, &loc2))
                .and_then(|s| jsonx::unescape(s).ok())
        })
        .unwrap_or_default()
}

/// Reemplaza un campo dentro de `settings`, creando el objeto si falta.
fn settings_upsert(raw: &str, key: &str, value: &str) -> String {
    match jsonx::find_key_value(raw, "settings", 0) {
        Some(loc) => {
            let inner = &raw[loc.val_start..loc.val_end];
            let patched = jsonx::upsert_string(inner, key, value);
            jsonx::replace_range(raw, loc.val_start, loc.val_end, &patched)
        }
        None => jsonx::insert_first_member(
            raw,
            &format!("\"settings\": {}", jsonx::upsert_string("{}", key, value)),
        ),
    }
}

impl Provider {
    /// Construye un proveedor desde su fragmento JSON crudo.
    pub fn from_raw(id: String, raw: String) -> Provider {
        let str_field = |key: &str| {
            jsonx::find_key_value(&raw, key, 0)
                .and_then(|loc| jsonx::string_value(&raw, &loc))
                .and_then(|s| jsonx::unescape(s).ok())
                .unwrap_or_default()
        };
        let models = jsonx::find_key_value(&raw, "models", 0)
            .map(|loc| raw[loc.val_start..loc.val_end].to_string())
            .unwrap_or_else(|| "{}".into());
        Provider {
            id,
            name: str_field("name"),
            package: str_field("package"),
            base_url: settings_string(&raw, "baseURL"),
            api_key: settings_string(&raw, "apiKey"),
            raw,
            models,
        }
    }

    /// Crea un proveedor nuevo desde los campos del formulario. `models` ya
    /// debe estar validado como objeto JSON.
    pub fn from_fields(
        id: &str,
        name: &str,
        package: &str,
        base_url: &str,
        api_key: &str,
        models: &str,
    ) -> Provider {
        let raw = format!(
            "{{\n  \"name\": {},\n  \"package\": {},\n  \"settings\": {{\n    \"baseURL\": {},\n    \"apiKey\": {}\n  }}\n}}",
            jsonx::json_string(name),
            jsonx::json_string(package),
            jsonx::json_string(base_url),
            jsonx::json_string(api_key),
        );
        let mut p = Provider {
            id: id.into(),
            raw,
            name: name.into(),
            package: package.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            models: "{}".into(),
        };
        p.set_models(models);
        p
    }

    pub fn set_name(&mut self, v: &str) {
        self.name = v.into();
        self.raw = jsonx::upsert_string(&self.raw, "name", v);
    }

    pub fn set_package(&mut self, v: &str) {
        self.package = v.into();
        self.raw = jsonx::upsert_string(&self.raw, "package", v);
    }

    pub fn set_base_url(&mut self, v: &str) {
        self.base_url = v.into();
        self.raw = settings_upsert(&self.raw, "baseURL", v);
    }

    pub fn set_api_key(&mut self, v: &str) {
        self.api_key = v.into();
        self.raw = settings_upsert(&self.raw, "apiKey", v);
    }

    /// Reemplaza el objeto `models` (v ya validado como JSON de objeto).
    pub fn set_models(&mut self, v: &str) {
        self.models = v.into();
        self.raw = match jsonx::find_key_value(&self.raw, "models", 0) {
            Some(loc) => jsonx::replace_range(&self.raw, loc.val_start, loc.val_end, v),
            None => jsonx::insert_first_member(&self.raw, &format!("\"models\": {v}")),
        };
    }

    /// Cambia el id (la clave). El cuerpo no se toca.
    pub fn rename(&mut self, new_id: &str) {
        self.id = new_id.into();
    }
}

/// Carpeta de config global de OpenCode: `~/.config/opencode`.
pub fn config_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("opencode")
}

/// Archivos de config que pueden llevar proveedores, en orden de prioridad.
fn candidate_files(dir: &Path) -> Vec<PathBuf> {
    let json = dir.join("opencode.json");
    let jsonc = dir.join("opencode.jsonc");
    let mut v = Vec::new();
    if json.is_file() {
        v.push(json);
    }
    if jsonc.is_file() {
        v.push(jsonc);
    }
    v
}

fn collect(text: &str, out: &mut Vec<Provider>) {
    let Some(loc) = jsonx::find_key_value(text, "providers", 0) else {
        return;
    };
    let obj = &text[loc.val_start..loc.val_end];
    jsonx::for_each_member(obj, |id, loc| {
        let raw = obj[loc.val_start..loc.val_end].to_string();
        out.push(Provider::from_raw(id.to_string(), raw));
    });
}

/// Lee la unión de proveedores de todos los archivos de config globales.
pub fn load_from(dir: &Path) -> Vec<Provider> {
    let mut out: Vec<Provider> = Vec::new();
    for path in candidate_files(dir) {
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut per_file: Vec<Provider> = Vec::new();
        collect(&text, &mut per_file);
        for p in per_file {
            let id = p.id.clone();
            if !out.iter().any(|x| x.id == id) {
                out.push(p);
            }
        }
    }
    out
}

/// Escribe la lista en la config global: union → `opencode.json`, y saca el
/// objeto `providers` de `opencode.jsonc` si existía.
pub fn save_into(dir: &Path, providers: &[Provider]) -> Result<String, String> {
    let json_path = dir.join("opencode.json");
    let body: Vec<String> = providers
        .iter()
        .map(|p| format!("{}: {}", jsonx::json_string(&p.id), p.raw.trim()))
        .collect();
    let providers_json = if body.is_empty() {
        "{}".to_string()
    } else {
        format!("{{\n  {}\n}}", body.join(",\n  "))
    };

    // `opencode.json`: se reemplaza (o inserta) el objeto `providers`.
    let mut text = match fs::read_to_string(&json_path) {
        Ok(t) if !t.trim().is_empty() => t,
        _ => "{\n  \"$schema\": \"https://opencode.ai/config.json\"\n}".to_string(),
    };
    match jsonx::find_key_value(&text, "providers", 0) {
        Some(loc) => {
            text = jsonx::replace_range(&text, loc.val_start, loc.val_end, &providers_json)
        }
        None => {
            let member = format!("\"providers\": {providers_json}");
            text = jsonx::insert_first_member(&text, &member);
        }
    }
    jsonx::parse(&text).map_err(|e| format!("la config resultante no es JSON válido: {e}"))?;
    if let Some(parent) = json_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&json_path, text)
        .map_err(|e| format!("no se pudo escribir {}: {e}", json_path.display()))?;

    // `opencode.jsonc`: se quita `providers` para que no queden copias viejas.
    let jsonc_path = dir.join("opencode.jsonc");
    if jsonc_path.is_file() {
        if let Ok(t) = fs::read_to_string(&jsonc_path) {
            if let Some(loc) = jsonx::find_key_value(&t, "providers", 0) {
                let t2 = jsonx::remove_member(&t, &loc);
                fs::write(&jsonc_path, t2)
                    .map_err(|e| format!("no se pudo actualizar {}: {e}", jsonc_path.display()))?;
            }
        }
    }

    Ok(format!(
        "{} proveedor(es) guardado(s) en {}",
        providers.len(),
        json_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Carpeta temporal única (la limpia el Drop).
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> TestDir {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!(
                "nuide-providers-test-{}-{}",
                std::process::id(),
                n
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).expect("crear temp dir");
            TestDir(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn union_de_dos_archivos_sin_duplicados() {
        let dir = TestDir::new();
        fs::write(
            dir.path().join("opencode.json"),
            r#"{
  "$schema": "https://opencode.ai/config.json",
  "providers": {
    "apinex": {
      "name": "APInex",
      "package": "@opencode/ai/providers/openai-compatible",
      "settings": { "baseURL": "https://api.apinex.bond/v1", "apiKey": "sk-1" },
      "models": { "free/a": { "name": "A", "cost": { "input": 0, "output": 0 } } }
    }
  }
}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("opencode.jsonc"),
            r#"{
  // otro archivo
  "providers": {
    "top-tool-ai": {
      "name": "Top Tools AI",
      "env": ["TOP_TOOLS_AI_API_KEY"],
      "package": "@opencode/ai/providers/openai-compatible",
      "settings": { "baseURL": "https://top-tools-ai.com/api/v1" }
    }
  }
}"#,
        )
        .unwrap();

        let list = load_from(dir.path());
        assert_eq!(list.len(), 2);
        let apinex = list.iter().find(|p| p.id == "apinex").unwrap();
        assert_eq!(apinex.base_url, "https://api.apinex.bond/v1");
        assert_eq!(apinex.api_key, "sk-1");
        assert!(apinex.models.contains("free/a"));
        let top = list.iter().find(|p| p.id == "top-tool-ai").unwrap();
        assert_eq!(top.name, "Top Tools AI");
        // El campo `env` desconocido se conserva en crudo.
        assert!(top.raw.contains("TOP_TOOLS_AI_API_KEY"));
    }

    #[test]
    fn editar_conserva_campos_desconocidos() {
        let dir = TestDir::new();
        fs::write(
            dir.path().join("opencode.json"),
            r#"{
  "providers": {
    "qwencloud": {
      "name": "Qwen Cloud",
      "env": ["QWENCLOUD_API_KEY"],
      "package": "@opencode/ai/providers/openai-compatible",
      "settings": { "baseURL": "https://x" }
    }
  }
}"#,
        )
        .unwrap();

        let mut list = load_from(dir.path());
        let p = list.iter_mut().find(|p| p.id == "qwencloud").unwrap();
        p.set_base_url("https://nuevo.example/v1");
        p.set_api_key("sk-99");
        p.set_name("Qwen Nuevo");
        p.set_models(r#"{ "qwen-max": { "name": "qwen-max" } }"#);
        p.rename("qwen2");

        save_into(dir.path(), &list).expect("guarda");
        let text = fs::read_to_string(dir.path().join("opencode.json")).unwrap();
        // Se conserva `env` aunque el CRUD no lo edite.
        assert!(text.contains("QWENCLOUD_API_KEY"));
        assert!(text.contains("https://nuevo.example/v1"));
        assert!(text.contains("sk-99"));
        assert!(text.contains("\"qwen2\""));
        assert!(!text.contains("\"qwencloud\""));

        let reloaded = load_from(dir.path());
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].id, "qwen2");
        assert_eq!(reloaded[0].api_key, "sk-99");
    }

    #[test]
    fn guardar_pasa_todo_a_json_y_limpia_jsonc() {
        let dir = TestDir::new();
        fs::write(
            dir.path().join("opencode.jsonc"),
            r#"{
  "$schema": "https://opencode.ai/config.json",
  "providers": { "viejo": { "name": "Viejo" } }
}"#,
        )
        .unwrap();
        let list = load_from(dir.path());
        assert_eq!(list.len(), 1);

        let mut list = load_from(dir.path());
        list[0].set_name("Nuevo");
        save_into(dir.path(), &list).unwrap();

        let json = fs::read_to_string(dir.path().join("opencode.json")).unwrap();
        assert!(json.contains("\"viejo\"") && json.contains("Nuevo"));
        // El jsonc queda sin `providers`.
        let jsonc = fs::read_to_string(dir.path().join("opencode.jsonc")).unwrap();
        assert!(!jsonc.contains("providers"));
        assert!(jsonc.contains("$schema"));
        // La union sigue valiendo 1.
        assert_eq!(load_from(dir.path()).len(), 1);
    }

    #[test]
    fn crear_y_agregar_proveedor_nuevo() {
        let dir = TestDir::new();
        fs::create_dir_all(dir.path()).unwrap();

        let p = Provider::from_fields(
            "nuevo.provider",
            "Nuevo",
            "@opencode/ai/providers/openai-compatible",
            "https://llm.example/v1",
            "sk-nuevo",
            r#"{ "m1": { "name": "M1" } }"#,
        );
        save_into(dir.path(), &[p]).expect("guarda");

        let text = fs::read_to_string(dir.path().join("opencode.json")).unwrap();
        assert!(text.contains("\"$schema\""));
        assert!(text.contains("\"nuevo.provider\""));
        assert!(text.contains("https://llm.example/v1"));

        let list = load_from(dir.path());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "nuevo.provider");
        assert_eq!(list[0].models, r#"{ "m1": { "name": "M1" } }"#);
    }
}
