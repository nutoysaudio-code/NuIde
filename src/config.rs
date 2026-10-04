//! Configuración mínima de NuIde persistida entre ejecuciones.
//!
//! Por ahora sólo recuerda la **carpeta del último proyecto**. El formato es
//! `clave=valor` en un archivo de texto chico, sin dependencias de parseo. No es
//! crítico: si el archivo no existe o no se puede escribir, se sigue andando con
//! la carpeta de lanzamiento.

use std::path::{Path, PathBuf};

/// Nombre del archivo dentro de la carpeta de configuración.
const FILE: &str = "config";
/// Clave de la carpeta del proyecto.
const KEY_PROJECT: &str = "project";

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

/// Último proyecto guardado, si el archivo existe y la carpeta sigue estando.
pub fn load_project() -> Option<PathBuf> {
    load_project_from(&config_dir())
}

/// Guarda la carpeta del proyecto. Falla en silencio: no vale la pena molestar
/// al usuario si no se puede escribir la config.
pub fn save_project(path: &Path) {
    save_project_to(&config_dir(), path)
}

fn load_project_from(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
    let path = parse_project(&text)?;
    path.is_dir().then_some(path)
}

fn save_project_to(dir: &Path, path: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(FILE), serialize_project(path));
}

/// Lee el valor de `project=` del archivo de config.
fn parse_project(text: &str) -> Option<PathBuf> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != KEY_PROJECT {
            return None;
        }
        let path = PathBuf::from(value.trim());
        (!path.as_os_str().is_empty()).then_some(path)
    })
}

/// Serializa la carpeta del proyecto al formato `clave=valor`.
fn serialize_project(path: &Path) -> String {
    format!("{KEY_PROJECT}={}\n", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nuide-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn parsea_y_serializa_la_carpeta_del_proyecto() {
        let dir = Path::new("F:\\proyectos\\coquito");
        let text = serialize_project(dir);
        assert_eq!(parse_project(&text).as_deref(), Some(dir));

        // Espacios alrededor y otras claves no molestan.
        let con_ruido = "tema=oscuro\nproject =  C:\\x  \n";
        assert_eq!(
            parse_project(con_ruido).as_deref(),
            Some(Path::new("C:\\x"))
        );
        assert_eq!(parse_project("tema=oscuro\n"), None);
    }

    #[test]
    fn guarda_y_recupera_solo_si_la_carpeta_existe() {
        let dir = temp_dir("roundtrip");
        let proyecto = dir.join("proyecto");
        std::fs::create_dir_all(&proyecto).unwrap();

        save_project_to(&dir, &proyecto);
        assert_eq!(load_project_from(&dir).as_deref(), Some(proyecto.as_path()));

        // Si la carpeta ya no está, se ignora la configuración.
        std::fs::remove_dir_all(&proyecto).unwrap();
        assert_eq!(load_project_from(&dir), None);

        // Y con un archivo vacío tampoco explota.
        std::fs::write(dir.join(FILE), "").unwrap();
        assert_eq!(load_project_from(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
