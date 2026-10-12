//! Imágenes para la charla multimodal: carga un archivo local y lo convierte a
//! un **data URL** (`data:<mime>;base64,…`) que viaja en el mensaje `user` de la
//! API `chat/completions` (formato OpenAI).
//!
//! Sin dependencias: base64 propio. Sólo formatos que los modelos de visión
//! aceptan (png/jpeg/webp/gif) y un tope de tamaño.

use std::path::Path;

/// Tope del archivo crudo (10 MB). El base64 ocupa ~33% más.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Una imagen lista para adjuntar.
#[derive(Debug, Clone)]
pub struct Image {
    /// Ruta de origen (para mostrar).
    pub label: String,
    /// MIME (`image/png`, …).
    pub mime: String,
    /// `data:<mime>;base64,<…>`.
    pub data_url: String,
}

/// Carga y codifica una imagen. Error legible si no sirve.
pub fn load(path: &Path) -> Result<Image, String> {
    let mime = mime_for(path).ok_or_else(|| {
        format!(
            "formato no soportado (png/jpg/jpeg/webp/gif): {}",
            path.display()
        )
    })?;
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "imagen muy grande ({:.1} MB; máximo {} MB)",
            meta.len() as f64 / 1_048_576.0,
            MAX_BYTES / 1_048_576
        ));
    }
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let data_url = format!("data:{mime};base64,{}", base64(&data));
    Ok(Image {
        label: path.display().to_string(),
        mime: mime.to_string(),
        data_url,
    })
}

fn mime_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

/// base64 estándar (RFC 4648) con relleno `=`.
fn base64(data: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHA[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHA[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// base64 estándar (RFC 4648) con relleno `=` a bytes. `None` si hay caracteres
/// inválidos.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' || c == b' ' {
            continue;
        }
        let v = val(c)?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodifica() {
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("Zm9v").unwrap(), b"foo");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
        assert!(base64_decode("****").is_none());
    }

    #[test]
    fn base64_conocidos() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn carga_png_y_arma_data_url() {
        let dir = std::env::temp_dir().join(format!("nucoder-img-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("x.png");
        std::fs::write(&f, b"foo").unwrap();
        let im = load(&f).unwrap();
        assert_eq!(im.mime, "image/png");
        assert_eq!(im.data_url, "data:image/png;base64,Zm9v");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rechaza_extension_desconocida() {
        assert!(load(Path::new("no.txt")).is_err());
    }
}
