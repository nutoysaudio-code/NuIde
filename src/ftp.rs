//! Cliente **FTP/FTPS básico** por `curl` (y SFTP por el mismo `curl`).
//!
//! Sin crates de red ni binarios extra: Windows ya trae `curl.exe`. Reusa
//! [`crate::site::Site`] (transporte, host, puerto, usuario, contraseña) y
//! ejecuta cada operación en un **hilo**, avisando por un canal —mismo patrón
//! que `deploy.rs`.
//!
//! Decisiones (ver `docs/FTP_CLIENT.md`):
//! - **FTPS explícito** (`ftp://` + `--ssl-reqd`) por defecto; el implícito sólo
//!   en el puerto 990 (lo resuelve [`Site::url_for`]).
//! - **Credenciales por archivo** (`curl -K`), nunca en la línea de comandos:
//!   así la contraseña no queda visible en la lista de procesos.
//! - Borrar/crear carpeta/renombrar van por comandos del protocolo (`-Q`, con
//!   `curl --quote`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::site::{Site, Transport};

/// Programa externo (el `curl.exe` que viene con Windows 10+).
fn curl() -> &'static str {
    if cfg!(windows) {
        "curl.exe"
    } else {
        "curl"
    }
}

/// Una entrada de un listado remoto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    /// ¿Es un directorio?
    pub dir: bool,
    /// Tamaño en bytes (si el formato del listado lo trae).
    pub size: Option<u64>,
}

/// Operación remota.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Listar un directorio (ruta absoluta del servidor).
    List { path: String },
    /// Bajar `remote` a `local`.
    Download { remote: String, local: PathBuf },
    /// Subir `local` a `remote`.
    Upload { local: PathBuf, remote: String },
    /// Borrar un archivo remoto.
    Delete { remote: String },
    /// Crear un directorio remoto.
    Mkdir { remote: String },
    /// Borrar un directorio remoto (vacío).
    Rmdir { remote: String },
    /// Renombrar/mover en el servidor.
    Rename { from: String, to: String },
}

/// Lo que el hilo le cuenta a la UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FtpMsg {
    /// Listado de `path` (entradas + texto crudo por si el formato es raro).
    Listed {
        path: String,
        entries: Vec<RemoteEntry>,
        raw: String,
    },
    /// Operación terminada (`detail` = stderr limpio, para errores).
    Done { ok: bool, detail: String },
    /// Cortocircuito (sin host, sin curl, etc.).
    Failed(String),
}

/// Normaliza una ruta remota a `/a/b` (barras `/`, resuelve `.`/`..`).
pub fn normalize_remote(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let owned = path.replace('\\', "/");
    for seg in owned.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Directorio padre de una ruta remota (normalizada).
pub fn parent_remote(path: &str) -> String {
    let norm = normalize_remote(path);
    match norm.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => norm[..i].to_string(),
    }
}

/// Une un directorio remoto con un nombre.
pub fn child_remote(dir: &str, name: &str) -> String {
    normalize_remote(&format!("{}/{}", normalize_remote(dir), name.trim_matches('/')))
}

/// Interpreta la salida de un `LIST` (formato Unix `ls -l` o MS-DOS). Si una
/// línea no encaja en ninguno, se toma como un nombre suelto (tolerante).
pub fn parse_listing(text: &str) -> Vec<RemoteEntry> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let t = line.trim();
        if t.is_empty() || t.to_ascii_lowercase().starts_with("total ") {
            continue;
        }
        if let Some(e) = parse_unix(line).or_else(|| parse_dos(line)) {
            out.push(e);
        } else {
            out.push(RemoteEntry {
                name: t.to_string(),
                dir: false,
                size: None,
            });
        }
    }
    out
}

/// `-rw-r--r-- 1 user group 1234 Jan  1 12:00 nombre`
fn parse_unix(line: &str) -> Option<RemoteEntry> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 9 {
        return None;
    }
    let perms = f[0].as_bytes();
    if perms.len() < 10 {
        return None;
    }
    let kind = perms[0] as char;
    if !matches!(kind, '-' | 'd' | 'l') {
        return None;
    }
    if !perms[1..10]
        .iter()
        .all(|c| matches!(*c as char, 'r' | 'w' | 'x' | 's' | 'S' | 't' | 'T' | '-'))
    {
        return None;
    }
    let size = f[4].parse::<u64>().ok();
    let name = f[8..].join(" ");
    if name.is_empty() {
        return None;
    }
    Some(RemoteEntry {
        name,
        dir: kind == 'd',
        size,
    })
}

/// `01-01-24  12:00PM       <DIR>          nombre` (o con tamaño).
fn parse_dos(line: &str) -> Option<RemoteEntry> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 4 {
        return None;
    }
    let date_ok = f[0].contains('-') || f[0].contains('/');
    let time_ok = f[1].contains(':') || f[1].to_ascii_uppercase().contains('M');
    if !date_ok || !time_ok {
        return None;
    }
    let dir = f[2].eq_ignore_ascii_case("<DIR>");
    let size = if dir {
        None
    } else {
        f[2].replace(',', "").parse::<u64>().ok()
    };
    let name = f[3..].join(" ");
    if name.is_empty() {
        return None;
    }
    Some(RemoteEntry { name, dir, size })
}

/// Escapa un valor para el archivo de config de curl.
fn cred_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Escribe las credenciales (y la CA de FTPS) en un archivo temporal `curl -K`.
/// Devuelve la ruta; quien llama debe borrarla al terminar.
fn cred_file(site: &Site) -> std::io::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("nuide-ftp-{}-{nanos}.cfg", std::process::id()));
    let mut f = std::fs::File::create(&path)?;
    if !site.user.is_empty() || !site.pass.is_empty() {
        writeln!(
            f,
            "user = \"{}:{}\"",
            cred_escape(&site.user),
            cred_escape(&site.pass)
        )?;
    }
    if site.transport == Transport::Ftps
        && !site.cacert.trim().is_empty()
        && site.cacert.trim() != "usa-scp"
    {
        writeln!(f, "cacert = \"{}\"", cred_escape(site.cacert.trim()))?;
    }
    Ok(path)
}

/// Opciones comunes a toda operación (`-sS`, credenciales, TLS explícito).
fn common_args(site: &Site, cred: &Path) -> Vec<String> {
    let mut args = vec![
        "-sS".to_string(),
        "--connect-timeout".to_string(),
        "20".to_string(),
        "-K".to_string(),
        cred.display().to_string(),
    ];
    if site.is_explicit_ftps() {
        args.push("--ssl-reqd".to_string());
    }
    args
}

/// URL de un directorio (con barra final).
fn dir_url(site: &Site, path: &str) -> String {
    let norm = normalize_remote(path);
    let rel = if norm == "/" {
        String::new()
    } else {
        format!("{}/", norm)
    };
    site.url_for(&rel)
}

/// URL de un archivo.
fn file_url(site: &Site, remote: &str) -> String {
    site.url_for(&normalize_remote(remote))
}

/// Argumentos de `curl` para listar un directorio.
pub fn list_args(site: &Site, path: &str, cred: &Path) -> Vec<String> {
    let mut args = common_args(site, cred);
    args.push(dir_url(site, path));
    args
}

/// Argumentos de `curl` para bajar un archivo.
pub fn download_args(site: &Site, remote: &str, local: &Path, cred: &Path) -> Vec<String> {
    let mut args = common_args(site, cred);
    args.push("-o".to_string());
    args.push(local.display().to_string());
    args.push(file_url(site, remote));
    args
}

/// Argumentos de `curl` para subir un archivo.
pub fn upload_args(site: &Site, local: &Path, remote: &str, cred: &Path) -> Vec<String> {
    let mut args = common_args(site, cred);
    args.push("--fail".to_string());
    args.push("-T".to_string());
    args.push(local.display().to_string());
    if matches!(site.transport, Transport::Ftp | Transport::Ftps) {
        args.push("--ftp-create-dirs".to_string());
    }
    args.push(file_url(site, remote));
    args
}

/// Argumentos para un comando de protocolo (`-Q`) contra la raíz del servidor.
fn quote_args(site: &Site, cred: &Path, commands: &[String]) -> Vec<String> {
    let mut args = common_args(site, cred);
    for cmd in commands {
        args.push("-Q".to_string());
        args.push(cmd.clone());
    }
    args.push(site.url_for(""));
    args
}

pub fn delete_args(site: &Site, remote: &str, cred: &Path) -> Vec<String> {
    quote_args(site, cred, &[format!("DELE {}", normalize_remote(remote))])
}

pub fn mkdir_args(site: &Site, remote: &str, cred: &Path) -> Vec<String> {
    quote_args(site, cred, &[format!("MKD {}", normalize_remote(remote))])
}

pub fn rmdir_args(site: &Site, remote: &str, cred: &Path) -> Vec<String> {
    quote_args(site, cred, &[format!("RMD {}", normalize_remote(remote))])
}

pub fn rename_args(site: &Site, from: &str, to: &str, cred: &Path) -> Vec<String> {
    quote_args(
        site,
        cred,
        &[
            format!("RNFR {}", normalize_remote(from)),
            format!("RNTO {}", normalize_remote(to)),
        ],
    )
}

/// Corre `curl` y devuelve `(ok, stdout, stderr)`.
fn run(args: &[String]) -> Result<(bool, String, String), String> {
    match crate::proc::hidden(curl()).args(args).output() {
        Ok(out) => Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        )),
        Err(e) => Err(format!("no se pudo correr {}: {e}", curl())),
    }
}

/// Ejecuta una operación en un hilo y avisa por `tx`.
pub fn spawn_op(site: Site, op: Op, tx: Sender<FtpMsg>) -> JoinHandle<()> {
    thread::spawn(move || {
        if !site.is_configured() {
            let _ = tx.send(FtpMsg::Failed(
                "el sitio no tiene host (completá la conexión)".into(),
            ));
            return;
        }
        let cred = match cred_file(&site) {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.send(FtpMsg::Failed(format!("no se pudo preparar las credenciales: {e}")));
                return;
            }
        };
        let msg = run_op(&site, &op, &cred);
        let _ = std::fs::remove_file(&cred);
        let _ = tx.send(msg);
    })
}

/// Corre la operación y arma el mensaje para la UI.
fn run_op(site: &Site, op: &Op, cred: &Path) -> FtpMsg {
    let args = match op {
        Op::List { path } => list_args(site, path, cred),
        Op::Download { remote, local } => download_args(site, remote, local, cred),
        Op::Upload { local, remote } => upload_args(site, local, remote, cred),
        Op::Delete { remote } => delete_args(site, remote, cred),
        Op::Mkdir { remote } => mkdir_args(site, remote, cred),
        Op::Rmdir { remote } => rmdir_args(site, remote, cred),
        Op::Rename { from, to } => rename_args(site, from, to, cred),
    };
    match run(&args) {
        Err(e) => FtpMsg::Failed(e),
        Ok((_, stdout, stderr)) => match op {
            Op::List { path } => {
                if stderr.is_empty() {
                    FtpMsg::Listed {
                        path: normalize_remote(path),
                        entries: parse_listing(&stdout),
                        raw: stdout,
                    }
                } else {
                    FtpMsg::Failed(stderr)
                }
            }
            _ => FtpMsg::Done {
                ok: stderr.is_empty(),
                detail: stderr,
            },
        },
    }
}

/// Ejecuta una operación pedida por el **agente** (NuCoder) y devuelve el texto
/// que verá el modelo. Corre en el hilo que la invoca (el IDE la llama en uno
/// aparte).
pub fn run_job(site: &Site, op: nucoder::tools::FtpOp) -> String {
    use nucoder::tools::FtpOp as O;
    let cred = match cred_file(site) {
        Ok(p) => p,
        Err(e) => return format!("error: no se pudo preparar las credenciales: {e}"),
    };
    let outcome = match &op {
        O::Open(_) => Ok("cliente FTP abierto en la IDE".to_string()),
        O::List(path) => match run(&list_args(site, path, &cred)) {
            Ok((_, stdout, stderr)) if stderr.is_empty() => {
                let n = parse_listing(&stdout).len();
                Ok(format!("{path}: {n} entrada(s)\n{stdout}"))
            }
            Ok((_, _, stderr)) => Err(stderr),
            Err(e) => Err(e),
        },
        O::Upload { local, remote } => simple(run(&upload_args(site, Path::new(local), remote, &cred))),
        O::Download { remote, local } => {
            simple(run(&download_args(site, remote, Path::new(local), &cred)))
        }
        O::Delete(remote) => simple(run(&delete_args(site, remote, &cred))),
        O::Mkdir(remote) => simple(run(&mkdir_args(site, remote, &cred))),
    };
    let _ = std::fs::remove_file(&cred);
    match outcome {
        Ok(s) => s,
        Err(e) => format!("error: {e}"),
    }
}

/// Respuesta para operaciones sin salida propia (subir/bajar/borrar/mkdir).
fn simple(result: Result<(bool, String, String), String>) -> Result<String, String> {
    match result {
        Ok((ok, _, stderr)) if ok && stderr.is_empty() => Ok("ok".to_string()),
        Ok((_, _, stderr)) if !stderr.is_empty() => Err(stderr),
        Ok(_) => Err("la operación falló".to_string()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site::DEFAULT_INCLUDE;
    fn site(transport: Transport) -> Site {
        Site {
            transport,
            host: "ftp.ejemplo.com".into(),
            remote_root: "/www".into(),
            include: DEFAULT_INCLUDE.into(),
            ..Site::default()
        }
    }

    #[test]
    fn normaliza_y_arma_rutas_remotas() {
        assert_eq!(normalize_remote("a\\b/c"), "/a/b/c");
        assert_eq!(normalize_remote("/a//b/./c"), "/a/b/c");
        assert_eq!(normalize_remote("/a/b/../c"), "/a/c");
        assert_eq!(normalize_remote("/"), "/");
        assert_eq!(normalize_remote(""), "/");
        assert_eq!(parent_remote("/a/b/c.txt"), "/a/b");
        assert_eq!(parent_remote("/a"), "/");
        assert_eq!(child_remote("/a", "b"), "/a/b");
        assert_eq!(child_remote("/", "b"), "/b");
    }

    #[test]
    fn parsea_listado_unix() {
        let raw = "total 8\n\
                   drwxr-xr-x 2 user group 4096 Jan  1 12:00 sub\n\
                   -rw-r--r-- 1 user group  123 Jan  1 12:00 hola mundo.txt\n\
                   lrwxrwxrwx 1 user group    5 Jan  1 12:00 link -> sub";
        let e = parse_listing(raw);
        assert_eq!(e.len(), 3, "{e:?}");
        assert!(e[0].dir && e[0].name == "sub");
        assert!(!e[1].dir && e[1].size == Some(123));
        assert_eq!(e[1].name, "hola mundo.txt");
        assert!(!e[2].dir && e[2].name.starts_with("link"));
    }

    #[test]
    fn parsea_listado_msdos() {
        let raw = "01-01-24  12:00PM       <DIR>          sub\n\
                   01-02-24  01:30AM                 4096 archivo.zip";
        let e = parse_listing(raw);
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e[0].dir && e[0].name == "sub");
        assert!(!e[1].dir && e[1].size == Some(4096) && e[1].name == "archivo.zip");
    }

    #[test]
    fn listado_desconocido_cae_a_nombres() {
        let e = parse_listing("uno\ndos\n");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "uno");
        assert!(!e[0].dir);
    }

    #[test]
    fn los_args_usan_archivo_de_credenciales_y_ssl_explicito() {
        let cred = Path::new("F:/tmp/cred.cfg");
        // FTP plano.
        let a = list_args(&site(Transport::Ftp), "/www", cred);
        assert!(a.join(" ").contains("-K F:/tmp/cred.cfg"), "{a:?}");
        assert!(a.join(" ").contains("ftp://ftp.ejemplo.com/www/"), "{a:?}");
        assert!(!a.join(" ").contains("--user"), "{a:?}");
        assert!(!a.join(" ").contains("--ssl-reqd"), "{a:?}");
        // FTPS explícito (puerto 21): `ftp://` + `--ssl-reqd`.
        let a = list_args(&site(Transport::Ftps), "/www", cred);
        assert!(a.join(" ").contains("--ssl-reqd"), "{a:?}");
        assert!(a.join(" ").contains("ftp://ftp.ejemplo.com/www/"), "{a:?}");
    }

    #[test]
    fn los_args_de_cada_operacion() {
        let cred = Path::new("cred.cfg");
        let up = upload_args(
            &site(Transport::Ftp),
            Path::new("F:/p/a.html"),
            "/www/a.html",
            cred,
        )
        .join(" ");
        assert!(up.contains("-T F:/p/a.html"), "{up}");
        assert!(up.contains("--ftp-create-dirs"), "{up}");

        let dl = download_args(&site(Transport::Ftp), "/www/a.html", Path::new("F:/o"), cred).join(" ");
        assert!(dl.contains("-o F:/o"), "{dl}");

        let del = delete_args(&site(Transport::Ftp), "/www/a.html", cred).join(" ");
        assert!(del.contains("-Q DELE /www/a.html"), "{del}");

        let mk = mkdir_args(&site(Transport::Ftp), "/www/nuevo", cred).join(" ");
        assert!(mk.contains("-Q MKD /www/nuevo"), "{mk}");

        let rn = rename_args(&site(Transport::Ftp), "/www/a", "/www/b", cred).join(" ");
        assert!(rn.contains("-Q RNFR /www/a"), "{rn}");
        assert!(rn.contains("-Q RNTO /www/b"), "{rn}");
    }
}
