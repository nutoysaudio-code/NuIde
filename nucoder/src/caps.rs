//! Capacidades de un modelo (texto, código, visión, imagen, audio, video,
//! embeddings, moderación).
//!
//! Se infieren, en este orden: de los metadatos del endpoint (OpenRouter trae
//! `architecture.input_modalities`/`output_modalities`/`modality`) y, si faltan,
//! de una heurística por id/nombre. También se pueden declarar a mano por modelo
//! (`"caps": ["text","code"]`).
//!
//! Nota: el coder habla `POST /chat/completions`, así que sólo los modelos con
//! capacidad `text` son ruteables; el resto sirve para **listar/filtrar** el
//! catálogo (los endpoints no-chat usan otras rutas).

use serde_json::Value;

/// Una capacidad de modelo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cap {
    Text,
    Code,
    Vision,
    Image,
    Audio,
    Video,
    Embedding,
    Guard,
}

/// Todas las capacidades en orden de presentación.
pub const ALL: [Cap; 8] = [
    Cap::Text,
    Cap::Code,
    Cap::Vision,
    Cap::Image,
    Cap::Audio,
    Cap::Video,
    Cap::Embedding,
    Cap::Guard,
];

/// Etiqueta de **especialidad** de un modelo a partir de sus capacidades (omite
/// "texto" si hay otras más específicas).
pub fn specialty(caps: &[Cap]) -> String {
    let specific: Vec<&str> = caps
        .iter()
        .filter(|c| **c != Cap::Text)
        .map(|c| c.label())
        .collect();
    if specific.is_empty() {
        caps.iter().map(|c| c.label()).collect::<Vec<_>>().join("+")
    } else {
        specific.join("+")
    }
}

impl Cap {
    pub fn as_str(self) -> &'static str {
        match self {
            Cap::Text => "text",
            Cap::Code => "code",
            Cap::Vision => "vision",
            Cap::Image => "image",
            Cap::Audio => "audio",
            Cap::Video => "video",
            Cap::Embedding => "embedding",
            Cap::Guard => "guard",
        }
    }

    /// Etiqueta legible de la **especialidad** (para el listado de modelos).
    pub fn label(self) -> &'static str {
        match self {
            Cap::Text => "texto",
            Cap::Code => "coder",
            Cap::Vision => "visión",
            Cap::Image => "imagen",
            Cap::Audio => "voz/audio",
            Cap::Video => "video",
            Cap::Embedding => "embedding",
            Cap::Guard => "guard",
        }
    }

    pub fn parse(s: &str) -> Option<Cap> {
        let s = s.trim().to_lowercase();
        ALL.into_iter()
            .find(|c| c.as_str() == s)
            .or(match s.as_str() {
                "texto" | "chat" | "llm" => Some(Cap::Text),
                "codigo" | "código" | "coder" => Some(Cap::Code),
                "vision" | "visión" | "vista" => Some(Cap::Vision),
                "imagen" | "img" => Some(Cap::Image),
                "voz" | "speech" => Some(Cap::Audio),
                "embed" => Some(Cap::Embedding),
                "moderation" | "moderacion" => Some(Cap::Guard),
                _ => None,
            })
    }
}

/// Infiere las capacidades de un modelo a partir de su `raw` y su nombre.
pub fn infer(id: &str, name: &str, raw: &Value) -> Vec<Cap> {
    let mut caps = from_payload(raw);
    for c in by_name(&format!("{id} {name}")) {
        if !caps.contains(&c) {
            caps.push(c);
        }
    }
    finalize(caps)
}

/// Capacidades declaradas explícitamente (`"caps": [...]`), si están.
pub fn explicit(v: &Value) -> Option<Vec<Cap>> {
    let arr = v.get("caps")?.as_array()?;
    let caps: Vec<Cap> = arr
        .iter()
        .filter_map(Value::as_str)
        .filter_map(Cap::parse)
        .collect();
    (!caps.is_empty()).then_some(caps)
}

fn finalize(mut caps: Vec<Cap>) -> Vec<Cap> {
    if caps.is_empty() {
        return vec![Cap::Text];
    }
    // Un modelo de texto/código/visión/audio suele ser chat-compatible; imagen,
    // video, embeddings y guard no.
    let chatty = caps
        .iter()
        .any(|c| matches!(c, Cap::Code | Cap::Vision | Cap::Audio));
    if chatty && !caps.contains(&Cap::Text) {
        caps.push(Cap::Text);
    }
    caps
}

fn from_payload(raw: &Value) -> Vec<Cap> {
    let mut out = Vec::new();
    let arch = raw.get("architecture");
    let arr = |v: &Value, k: &str| -> Vec<String> {
        v.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.to_lowercase())
                    .collect()
            })
            .unwrap_or_default()
    };

    let (mut ins, mut outs) = (Vec::new(), Vec::new());
    if let Some(a) = arch {
        ins = arr(a, "input_modalities");
        outs = arr(a, "output_modalities");
        if ins.is_empty() && outs.is_empty() {
            if let Some(m) = a.get("modality").and_then(Value::as_str) {
                // "text+image->text" o "text -> image"
                if let Some((l, r)) = m.split_once("->") {
                    ins = split_mods(l);
                    outs = split_mods(r);
                }
            }
        }
    }

    let has = |v: &[String], s: &str| v.iter().any(|x| x.contains(s));
    if has(&outs, "image") {
        out.push(Cap::Image);
    }
    if has(&outs, "video") {
        out.push(Cap::Video);
    }
    if has(&outs, "audio") {
        out.push(Cap::Audio);
    }
    if has(&ins, "image") {
        out.push(Cap::Vision);
    }
    if has(&ins, "audio") {
        out.push(Cap::Audio);
    }
    if has(&outs, "embedding") || has(&ins, "embedding") {
        out.push(Cap::Embedding);
    }
    if has(&ins, "text") || has(&outs, "text") {
        out.push(Cap::Text);
    }
    out
}

fn split_mods(s: &str) -> Vec<String> {
    s.split(['+', ','])
        .map(|x| x.trim().to_lowercase())
        .filter(|x| !x.is_empty())
        .collect()
}

fn by_name(s: &str) -> Vec<Cap> {
    let l = s.to_lowercase();
    let any = |keys: &[&str]| keys.iter().any(|k| l.contains(k));
    let toks: Vec<&str> = l
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let mut c = Vec::new();
    if any(&[
        "diffusion",
        "sdxl",
        "sd3",
        "stable-diffusion",
        "flux",
        "dall-e",
        "dalle",
        "imagen",
        "kandinsky",
        "playground-v",
        "janus",
        "consistory",
    ]) {
        c.push(Cap::Image);
    }
    if any(&[
        "video",
        "sora",
        "veo", // "veo" puede aparecer en otras palabras; se acota abajo
        "runway",
        "ltx",
        "svd",
        "animate",
        "cogvideo",
        "wan2",
        "hunyuan-video",
        "mochi",
    ]) {
        c.push(Cap::Video);
    }
    if any(&[
        "whisper", "tts", "speech", "audio", "voice", "bark", "musicgen", "parakeet", "canary",
        "wav2vec", "asr", "riva", "magpie", "voxtral",
    ]) {
        c.push(Cap::Audio);
    }
    if any(&["embed", "bge", "e5-", "gte-", "nomic-embed", "rerank"]) {
        c.push(Cap::Embedding);
    }
    if any(&["guard", "moderation", "safety", "shield"]) {
        c.push(Cap::Guard);
    }
    if any(&[
        "vision",
        "llava",
        "pixtral",
        "paligemma",
        "moondream",
        "fuyu",
        "kosmos",
        "deplot",
        "idefics",
        "vila",
        "multimodal",
        "omni",
        "-vl",
        "vl-",
        "qwen2-vl",
        "llama-4-scout",
        "llama-4-maverick",
        "gemma-3",
    ]) || toks.contains(&"vl")
        || toks.contains(&"vlm")
        || toks.contains(&"vision")
    {
        c.push(Cap::Vision);
    }
    if any(&[
        "coder",
        "codestral",
        "starcoder",
        "codegemma",
        "devstral",
        "codellama",
        "magicoder",
        "code-",
    ]) {
        c.push(Cap::Code);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn payload_openrouter_texto() {
        let raw = json!({"architecture": {
            "input_modalities": ["text"], "output_modalities": ["text"]
        }});
        assert_eq!(infer("x/y", "Y", &raw), vec![Cap::Text]);
    }

    #[test]
    fn payload_openrouter_vision() {
        let raw = json!({"architecture": {
            "modality": "text+image->text"
        }});
        let caps = infer("x/vl", "VL", &raw);
        assert!(caps.contains(&Cap::Vision));
        assert!(caps.contains(&Cap::Text));
    }

    #[test]
    fn heuristicas_por_nombre() {
        assert!(infer("stability/sdxl", "SDXL", &json!(null)).contains(&Cap::Image));
        assert!(infer("openai/whisper-large", "Whisper", &json!(null)).contains(&Cap::Audio));
        assert!(infer("qwen/qwen-coder", "Qwen Coder", &json!(null)).contains(&Cap::Code));
        assert!(infer("x/y", "modelo común", &json!(null)) == vec![Cap::Text]);
        assert!(infer("a/b", "bge-m3", &json!(null)).contains(&Cap::Embedding));
    }

    #[test]
    fn explicito_gana() {
        let v = json!({"caps": ["image"]});
        assert_eq!(explicit(&v), Some(vec![Cap::Image]));
    }
}
