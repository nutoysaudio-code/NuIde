//! Plantillas de archivos SpASM.
//!
//! Contenido mínimo y **probado** por el toolchain: el dialecto compilado usa
//! `programa`/`func`/`main`, y el de intérprete `print`/`end`.

/// Una plantilla de archivo `.spasm`.
pub struct Template {
    pub id: &'static str,
    pub label: &'static str,
    pub body: &'static str,
}

/// Plantillas disponibles (en el orden en que se muestran). El **desarrollo** se
/// hace con el intérprete (`print ...; end;`, se corre con F5); el **release** es
/// el dialecto compilado (`programa`/`main`, se compila con Ctrl+B).
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "desarrollo",
        label: "Desarrollo (intérprete) — F5",
        body: "; programa SpASM (desarrollo, dialecto de intérprete)\n\nprint \"Hola desde SpASM\";\nend;\n",
    },
    Template {
        id: "release",
        label: "Release (compilado) — Ctrl+B",
        body: "; programa SpASM (release, dialecto compilado)\n\nprograma \"nuevo\"\n\nfunc suma(a : int) -> int {\n    let b = a\n    return b\n}\n\nmain {\n    let c = suma(1)\n}\n",
    },
    Template {
        id: "vacio",
        label: "Archivo vacío",
        body: "; nuevo archivo SpASM\n",
    },
];

/// Plantilla por id (para los comandos de la paleta).
pub fn by_id(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hay_plantillas_y_tienen_contenido() {
        assert!(!TEMPLATES.is_empty());
        for t in TEMPLATES {
            assert!(!t.id.is_empty());
            assert!(!t.label.is_empty());
            assert!(t.body.contains(".spasm") || !t.body.is_empty());
        }
        assert!(by_id("desarrollo").is_some());
        assert!(by_id("release").is_some());
        assert!(by_id("no-existe").is_none());
    }
}
