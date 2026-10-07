//! Configuración de NuIde persistida entre ejecuciones.
//!
//! Un *store* mínimo `clave=valor` en un archivo de texto. Guarda la carpeta del
//! proyecto, qué paneles están visibles, la geometría de la ventana y las
//! pestañas abiertas. No es crítico: si el archivo no existe o no se puede
//! escribir, la IDE arranca con los valores por defecto.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Nombre del archivo dentro de la carpeta de configuración.
const FILE: &str = "config";

/// Configuración cargada en memoria. Se modifica con `set` y se vuelca con
/// `save`.
pub struct Config {
    values: BTreeMap<String, String>,
    dir: PathBuf,
}

impl Config {
    /// Lee el archivo de configuración del usuario (si existe). En las pruebas
    /// se ignora para que el estado real de la máquina no afecte los tests.
    pub fn load() -> Self {
        if cfg!(test) {
            return Self {
                values: BTreeMap::new(),
                dir: config_dir(),
            };
        }
        let dir = config_dir();
        let values = std::fs::read_to_string(dir.join(FILE))
            .ok()
            .map(|text| parse(&text))
            .unwrap_or_default();
        Self { values, dir }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some("1") | Some("true") => true,
            Some("0") | Some("false") => false,
            _ => default,
        }
    }

    pub fn f32(&self, key: &str, default: f32) -> f32 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn i32(&self, key: &str, default: i32) -> i32 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn usize(&self, key: &str, default: usize) -> usize {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn set(&mut self, key: &str, value: impl std::fmt::Display) {
        self.values.insert(key.to_string(), value.to_string());
    }

    /// Escribe el archivo. Falla en silencio.
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.dir.join(FILE), serialize(&self.values));
    }

    /// Carpeta del proyecto recordada, si sigue existiendo.
    pub fn project(&self) -> Option<PathBuf> {
        let path = PathBuf::from(self.get("project")?);
        path.is_dir().then_some(path)
    }
}

/// Carpeta de configuración del usuario: `%APPDATA%\NuIde` en Windows y
/// `$XDG_CONFIG_HOME/nuide` (o `~/.config/nuide`) en el resto. Si no hay ninguna
/// variable de entorno, cae a `.nuide` en el directorio actual.
pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("NuIde");
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("nuide");
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".config").join("nuide");
        }
    }
    PathBuf::from(".nuide")
}

/// Parsea `clave=valor` por línea. Ignora líneas vacías y comentarios.
///
/// El análisis de cada línea vive en **SpASM** (`spasm/configkv.spasm`);
/// este bucle sólo itera y arma el mapa. Ver `crate::spasm_logic`.
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if crate::spasm_logic::config_skip(line) {
            continue;
        }
        if let Some((key, value)) = crate::spasm_logic::config_split(line) {
            map.insert(key, value);
        }
    }
    map
}

/// Serializa el store, una clave por línea. Se queda en Rust: es pura
/// concatenación (sin lógica) y el concat del backend C trunca en 8 KiB, lo
/// que podría cortar la lista de pestañas abiertas.
fn serialize(values: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (key, value) in values {
        out.push_str(key);
        out.push('=');
        out.push_str(value);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_y_serializa_clave_valor() {
        let mut values = BTreeMap::new();
        values.insert("project".to_string(), "F:\\proyectos\\coquito".to_string());
        values.insert("show.left".to_string(), "0".to_string());
        let text = serialize(&values);
        let back = parse(&text);
        assert_eq!(
            back.get("project").map(String::as_str),
            Some("F:\\proyectos\\coquito")
        );
        assert_eq!(back.get("show.left").map(String::as_str), Some("0"));

        // Comentarios y espacios no molestan.
        let con_ruido = "# comentario\n\n show.right = 1 \nproject = C:\\x\n";
        let map = parse(con_ruido);
        assert_eq!(map.get("show.right").map(String::as_str), Some("1"));
        assert_eq!(map.get("project").map(String::as_str), Some("C:\\x"));
    }

    #[test]
    fn los_tipos_tienen_default_y_parsean() {
        let mut values = BTreeMap::new();
        values.insert("win.w".to_string(), "1280".to_string());
        values.insert("show.left".to_string(), "0".to_string());
        values.insert("tabs".to_string(), "2".to_string());
        let cfg = Config {
            values,
            dir: PathBuf::from("."),
        };
        assert_eq!(cfg.f32("win.w", 0.0), 1280.0);
        assert!(!cfg.bool("show.left", true));
        assert!(cfg.bool("show.right", true)); // default
        assert_eq!(cfg.usize("tabs", 0), 2);
        assert_eq!(cfg.usize("falta", 7), 7);
    }

    #[test]
    fn guarda_y_recupera_solo_si_la_carpeta_del_proyecto_existe() {
        let dir = std::env::temp_dir().join(format!("nuide-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proyecto = dir.join("proyecto");
        std::fs::create_dir_all(&proyecto).unwrap();

        let mut cfg = Config {
            values: BTreeMap::new(),
            dir: dir.clone(),
        };
        cfg.set("project", proyecto.display());
        cfg.save();
        let loaded = Config {
            values: parse(&std::fs::read_to_string(dir.join(FILE)).unwrap()),
            dir: dir.clone(),
        };
        assert_eq!(loaded.project().as_deref(), Some(proyecto.as_path()));

        // Si la carpeta ya no está, se ignora.
        std::fs::remove_dir_all(&proyecto).unwrap();
        assert_eq!(loaded.project(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
