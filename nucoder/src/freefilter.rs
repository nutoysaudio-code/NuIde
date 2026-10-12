//! Filtro de "gratis" por proveedor y presets de alta.
//!
//! Cada endpoint señala los gratuitos distinto (o no los señala). `FreeFilter`
//! decide qué modelo es gratis a partir del `raw` (precios), del nombre y del
//! flag `free_only` del proveedor. El resultado se persiste como `free` explícito
//! por modelo, que es lo que consume el router.
//!
//! Ver `nucoder/DESIGN.md` (sección «Filtro de "gratis" por proveedor»).

use serde_json::Value;

use crate::tools::glob_match;

/// Cómo decidir si un modelo es gratuito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeFilter {
    /// Precios → nombre → `free_only`.
    Auto,
    /// Todos los del catálogo son gratis.
    All,
    /// Ninguno.
    None,
    /// Por marcador en id/nombre.
    Name,
    /// Por precio 0 (auto-detecta las variantes comunes).
    Pricing,
    /// Por patrón glob en id/nombre (`free_match`).
    List,
}

/// Marcadores por defecto (substring) para `Name` y base de `List`.
pub const DEFAULT_MATCH: [&str; 5] = [":free", "(free)", "free", "gratis", "trial"];

impl FreeFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            FreeFilter::Auto => "auto",
            FreeFilter::All => "all",
            FreeFilter::None => "none",
            FreeFilter::Name => "name",
            FreeFilter::Pricing => "pricing",
            FreeFilter::List => "list",
        }
    }

    pub fn parse(s: &str) -> Option<FreeFilter> {
        match s.trim().to_lowercase().as_str() {
            "auto" => Some(FreeFilter::Auto),
            "all" | "all-free" | "todos" => Some(FreeFilter::All),
            "none" | "ninguno" => Some(FreeFilter::None),
            "name" | "nombre" => Some(FreeFilter::Name),
            "pricing" | "precio" => Some(FreeFilter::Pricing),
            "list" | "lista" | "glob" => Some(FreeFilter::List),
            _ => None,
        }
    }
}

/// Decide si un modelo es gratis según el filtro.
pub fn classify(
    filter: FreeFilter,
    raw: &Value,
    id: &str,
    name: &str,
    free_match: &[String],
    provider_free_only: bool,
) -> bool {
    match filter {
        FreeFilter::All => true,
        FreeFilter::None => false,
        FreeFilter::Name => name_hit(id, name, free_match),
        FreeFilter::List => list_hit(id, name, free_match),
        FreeFilter::Pricing => prices_sum(raw).map(|p| p == 0.0).unwrap_or(false),
        FreeFilter::Auto => {
            if let Some(p) = prices_sum(raw) {
                return p == 0.0;
            }
            if name_hit(id, name, free_match) {
                return true;
            }
            provider_free_only
        }
    }
}

/// ¿El id/nombre trae un marcador de gratis?
pub fn name_hit(id: &str, name: &str, free_match: &[String]) -> bool {
    let hay = format!("{id} {name}").to_lowercase();
    if free_match.is_empty() {
        DEFAULT_MATCH.iter().any(|m| hay.contains(m))
    } else {
        free_match.iter().any(|m| hay.contains(&m.to_lowercase()))
    }
}

fn list_hit(id: &str, name: &str, free_match: &[String]) -> bool {
    let pats: Vec<String> = if free_match.is_empty() {
        DEFAULT_MATCH.iter().map(|m| format!("*{m}*")).collect()
    } else {
        free_match.to_vec()
    };
    pats.iter()
        .any(|p| glob_match(p, id) || glob_match(p, name) || glob_match(&format!("*{p}*"), id))
}

/// Suma de precios (USD/token) si el `raw` trae estructura de precios.
pub fn prices_sum(raw: &Value) -> Option<f64> {
    let p = raw.get("pricing")?;
    if !p.is_object() {
        return None;
    }
    let num = |k: &str| match p.get(k) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    };
    let known = ["prompt", "completion", "input", "output"];
    let mut any = false;
    let mut sum = 0.0;
    for k in known {
        if let Some(v) = num(k) {
            any = true;
            sum += v;
        }
    }
    any.then_some(sum)
}

// ---------------------------------------------------------------------------
// Presets de proveedores
// ---------------------------------------------------------------------------

/// Un proveedor conocido: base URL y filtro de gratis por defecto.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub free_filter: FreeFilter,
}

const PRESETS: &[Preset] = &[
    Preset {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        free_filter: FreeFilter::Auto,
    },
    Preset {
        id: "nvidia",
        name: "NVIDIA NIM",
        base_url: "https://integrate.api.nvidia.com/v1",
        free_filter: FreeFilter::All,
    },
    Preset {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        free_filter: FreeFilter::All,
    },
    Preset {
        id: "cerebras",
        name: "Cerebras",
        base_url: "https://api.cerebras.ai/v1",
        free_filter: FreeFilter::All,
    },
    Preset {
        id: "gemini",
        name: "Google Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        free_filter: FreeFilter::All,
    },
    Preset {
        id: "mistral",
        name: "Mistral",
        base_url: "https://api.mistral.ai/v1",
        free_filter: FreeFilter::All,
    },
    Preset {
        id: "together",
        name: "Together",
        base_url: "https://api.together.xyz/v1",
        free_filter: FreeFilter::Pricing,
    },
    Preset {
        id: "deepinfra",
        name: "DeepInfra",
        base_url: "https://api.deepinfra.com/v1/openai",
        free_filter: FreeFilter::Pricing,
    },
    Preset {
        id: "fireworks",
        name: "Fireworks",
        base_url: "https://api.fireworks.ai/inference/v1",
        free_filter: FreeFilter::Pricing,
    },
    Preset {
        id: "sambanova",
        name: "SambaNova",
        base_url: "https://api.sambanova.ai/v1",
        free_filter: FreeFilter::All,
    },
    Preset {
        id: "hyperbolic",
        name: "Hyperbolic",
        base_url: "https://api.hyperbolic.xyz/v1",
        free_filter: FreeFilter::Pricing,
    },
];

/// Busca un preset por id (tolera el nombre del proveedor).
pub fn preset(id: &str) -> Option<Preset> {
    let id = id.trim().to_lowercase();
    let id = match id.as_str() {
        "nvidea" | "nv" | "nim" => "nvidia",
        "google" | "google-gemini" => "gemini",
        "together-ai" => "together",
        "deep-infra" => "deepinfra",
        other => other,
    };
    PRESETS.iter().copied().find(|p| p.id == id)
}

/// Filtro de gratis por defecto para un id (preset o `Auto`).
pub fn default_filter(id: &str) -> FreeFilter {
    preset(id)
        .map(|p| p.free_filter)
        .unwrap_or(FreeFilter::Auto)
}

/// Todos los presets (para `provider presets`).
pub fn all() -> &'static [Preset] {
    PRESETS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_usa_precios() {
        let free = json!({"pricing": {"prompt": "0", "completion": "0"}});
        let paid = json!({"pricing": {"prompt": "0.001", "completion": "0.002"}});
        assert!(classify(FreeFilter::Auto, &free, "x", "X", &[], false));
        assert!(!classify(FreeFilter::Auto, &paid, "x", "X", &[], false));
    }

    #[test]
    fn auto_cae_a_nombre_y_free_only() {
        assert!(classify(
            FreeFilter::Auto,
            &json!(null),
            "a/b:free",
            "B",
            &[],
            false
        ));
        assert!(classify(
            FreeFilter::Auto,
            &json!(null),
            "a/b",
            "B",
            &[],
            true
        ));
        assert!(!classify(
            FreeFilter::Auto,
            &json!(null),
            "a/b",
            "B",
            &[],
            false
        ));
    }

    #[test]
    fn all_y_none() {
        assert!(classify(
            FreeFilter::All,
            &json!(null),
            "a",
            "B",
            &[],
            false
        ));
        assert!(!classify(
            FreeFilter::None,
            &json!(null),
            "a:free",
            "B",
            &[],
            true
        ));
    }

    #[test]
    fn list_con_glob() {
        let m = vec!["nvidia/*".to_string()];
        assert!(classify(
            FreeFilter::List,
            &json!(null),
            "nvidia/llama",
            "L",
            &m,
            false
        ));
        assert!(!classify(
            FreeFilter::List,
            &json!(null),
            "meta/llama",
            "L",
            &m,
            false
        ));
    }

    #[test]
    fn presets_mapean_ids() {
        assert_eq!(preset("openrouter").unwrap().free_filter, FreeFilter::Auto);
        assert_eq!(preset("nvidea").unwrap().id, "nvidia");
        assert_eq!(preset("nvidia").unwrap().free_filter, FreeFilter::All);
    }
}
