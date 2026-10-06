//! Ejecuta un despliegue: corre el script SpASM del proyecto y hace las
//! transferencias con `curl` (FTP/FTPS/SFTP) o `scp` (OpenSSH).
//!
//! Todo el trabajo pesado va en un hilo: el canal manda [`DeployMsg`]s que
//! `editor::Ide::drain` muestra en la sesión `build` de la terminal. La UI nunca
//! se bloquea esperando la red.
//!
//! El script (por defecto `deploy.spasm` en la raíz del proyecto) se corre con el
//! intérprete que ya detecta [`crate::spasm`] — igual que F5:
//! `spasm.exe deploy.spasm`. De su stdout sólo se interpretan las líneas del
//! protocolo ([`crate::site::parse_script_line`]); el resto se muestra tal cual,
//! así el script puede ir contando lo que hace.
//!
//! Si el proyecto no tiene script, se despliega el `local_sub` completo filtrando
//! por extensión ([`Site::extensions`]): el caso típico de "subí el sitio".

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use crate::site::{parse_script_line, DeployEvent, Site, Transport};
use crate::spasm::Toolchain;

/// `curl.exe` viene en Windows 10+ y habla ftp/ftps/sftp.
pub const CURL: &str = "curl.exe";
/// `scp.exe` es el de OpenSSH (también en System32).
pub const SCP: &str = "scp.exe";
/// Carpetas que nunca se ofrecen para subir.
const SKIP_DIRS: &[&str] = &[".git", ".svn", "target", "node_modules", ".nuide"];

/// Lo que el hilo de despliegue le cuenta a la UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployMsg {
    /// Una línea para la terminal de build.
    Log(String),
    /// Resultado de subir un archivo.
    Sent { path: String, ok: bool },
    /// Cortocircuito: no se puede continuar (sin host, sin curl, ...).
    Aborted(String),
    /// Fin: subidos, saltados, fallados.
    Finished {
        sent: usize,
        skipped: usize,
        failed: usize,
    },
}

/// Una transferencia pedida: archivo local y ruta remota ya resueltas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub local: PathBuf,
    pub remote: String,
}

/// Archivos bajo `root` que pasan el filtro de extensiones, con la ruta relativa
/// a `root` (que es la que se usa para armar la ruta remota).
pub fn collect(root: &Path, site: &Site) -> Vec<Transfer> {
    let exts = site.extensions();
    let mut out = Vec::new();
    walk(root, root, &exts, &mut out);
    out.sort_by(|a, b| a.remote.cmp(&b.remote));
    out
}

fn walk(root: &Path, dir: &Path, exts: &[String], out: &mut Vec<Transfer>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, exts, out);
            continue;
        }
        let Some(ext) = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
        else {
            continue;
        };
        if !exts.contains(&ext) {
            continue;
        }
        // `rel` pide prestado `path`, así que se materializa primero y recién
        // después se mueve el `PathBuf` a la estructura.
        let remote = path
            .strip_prefix(root)
            .map(|rel| rel.display().to_string().replace('\\', "/"));
        let Ok(remote) = remote else { continue };
        out.push(Transfer {
            local: path,
            remote,
        });
    }
}

/// Traduce las líneas del script a transferencias concretas. `SKIP` y los
/// `UPLOAD` de archivos que no existen se descartan acá, no en el hilo.
pub fn plan(events: &[DeployEvent], root: &Path, site: &Site) -> (Vec<Transfer>, usize) {
    let mut transfers = Vec::new();
    let mut skipped = 0usize;
    for event in events {
        match event {
            DeployEvent::Upload { local, remote } => {
                let rel = PathBuf::from(local.trim().replace('\\', "/"));
                let full = if rel.is_absolute() {
                    rel.clone()
                } else {
                    root.join(&rel)
                };
                if !full.is_file() {
                    skipped += 1;
                    continue;
                }
                let dest = if remote.trim().is_empty() {
                    site.remote_for(&rel)
                } else {
                    site.remote_for(Path::new(remote.trim().replace('\\', "/").as_str()))
                };
                transfers.push(Transfer {
                    local: full,
                    remote: dest,
                });
            }
            DeployEvent::Skip { .. } => skipped += 1,
            _ => {}
        }
    }
    (transfers, skipped)
}

/// Argumentos de la transferencia para un archivo.
fn transfer_args(site: &Site, t: &Transfer) -> (String, Vec<String>) {
    if site.transport == Transport::Sftp && site.cacert.trim() == "usa-scp" {
        // Salida explícita por scp (el host SFTP no acepta bien el modo de curl).
        (SCP.to_string(), site.scp_args(&t.local, &t.remote))
    } else {
        (CURL.to_string(), site.curl_args(&t.local, &t.remote))
    }
}

/// Corre una transferencia y devuelve `ok` + el detalle para la terminal.
fn transfer_one(site: &Site, t: &Transfer) -> (bool, String) {
    let (program, args) = transfer_args(site, t);
    match Command::new(&program).args(&args).output() {
        Ok(out) => {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            (out.status.success(), detail)
        }
        Err(e) => (false, format!("no se pudo correr {program}: {e}")),
    }
}

/// Prueba el sitio: lista el directorio remoto con curl. Sirve para validar
/// host, credenciales y (en FTPS) el certificado sin subir nada.
pub fn spawn_test(site: Site, tx: Sender<DeployMsg>) -> JoinHandle<()> {
    thread::spawn(move || {
        if !site.is_configured() {
            let _ = tx.send(DeployMsg::Aborted(
                "el sitio no tiene host: completá Preferencias → Despliegue".into(),
            ));
            return;
        }
        let url = site.url_for("");
        let mut args = vec!["-sS".to_string(), "--list-only".to_string()];
        if !site.user.is_empty() {
            args.push("--user".to_string());
            args.push(format!("{}:{}", site.user, site.pass));
        }
        if site.transport == Transport::Ftps && !site.cacert.trim().is_empty() {
            args.push("--cacert".to_string());
            args.push(site.cacert.trim().to_string());
        }
        args.push(url.clone());
        let _ = tx.send(DeployMsg::Log(format!(
            "probando {} sobre {} ...",
            site.host,
            site.transport.label()
        )));
        match Command::new(CURL).args(&args).output() {
            Ok(out) if out.status.success() => {
                let lines = String::from_utf8_lossy(&out.stdout).lines().count();
                let _ = tx.send(DeployMsg::Log(format!(
                    "conexión OK: {url} ({lines} entradas)"
                )));
                let _ = tx.send(DeployMsg::Finished {
                    sent: 0,
                    skipped: 0,
                    failed: 0,
                });
            }
            Ok(out) => {
                let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let _ = tx.send(DeployMsg::Aborted(format!(
                    "falló la prueba contra {url}: {}",
                    if detail.is_empty() {
                        format!("código {}", out.status)
                    } else {
                        detail
                    }
                )));
            }
            Err(e) => {
                let _ = tx.send(DeployMsg::Aborted(format!(
                    "no se encontró {CURL} (venía con Windows): {e}"
                )));
            }
        }
    })
}

/// Corre el script del proyecto (o el escaneo por extensión) y sube lo que pida.
pub fn spawn_deploy(
    project: PathBuf,
    site: Site,
    toolchain: Toolchain,
    tx: Sender<DeployMsg>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        if !site.is_configured() {
            let _ = tx.send(DeployMsg::Aborted(
                "el sitio no tiene host: completá Preferencias → Despliegue".into(),
            ));
            return;
        }
        let root = site.local_root(&project);
        let (transfers, skipped) = match site.script_path(&project) {
            Some(script) => match run_script(&toolchain, &script, &tx) {
                Some(events) => {
                    let (t, s) = plan(&events, &root, &site);
                    (t, s)
                }
                // El script falló: aborta `run_script`, que ya avisó.
                None => (Vec::new(), 0),
            },
            None => {
                let _ = tx.send(DeployMsg::Log(format!(
                    "sin script de despliegue: se sube {} filtrando por extensión",
                    root.display()
                )));
                (collect(&root, &site), 0)
            }
        };

        let total = transfers.len();
        let _ = tx.send(DeployMsg::Log(format!(
            "{} archivo(s) para subir a {} ({})",
            total,
            site.host,
            site.transport.label()
        )));

        let mut sent = 0usize;
        let mut failed = 0usize;
        for t in transfers {
            let (ok, detail) = transfer_one(&site, &t);
            if ok {
                sent += 1;
            } else {
                failed += 1;
                if !detail.is_empty() {
                    let _ = tx.send(DeployMsg::Log(format!(
                        "  {}",
                        detail.lines().next().unwrap_or("")
                    )));
                }
            }
            let _ = tx.send(DeployMsg::Sent {
                path: t.remote.clone(),
                ok,
            });
        }
        if skipped > 0 {
            let _ = tx.send(DeployMsg::Log(format!("saltados: {skipped}")));
        }
        let _ = tx.send(DeployMsg::Finished {
            sent,
            skipped,
            failed,
        });
    })
}

/// Corre el script y devuelve los eventos del protocolo. `None` si no se pudo
/// correr (sin intérprete o proceso muerto); los mensajes sueltos del script se
/// reenvían como `Log`.
fn run_script(
    toolchain: &Toolchain,
    script: &Path,
    tx: &Sender<DeployMsg>,
) -> Option<Vec<DeployEvent>> {
    let Ok(exe) = toolchain.interpreter_required() else {
        let _ = tx.send(DeployMsg::Aborted(
            "no se encontró el intérprete spasm.exe (definí NUIDE_SPASM) para correr el script"
                .into(),
        ));
        return None;
    };
    let _ = tx.send(DeployMsg::Log(format!(
        "corriendo {} ...",
        script.file_name().unwrap_or_default().to_string_lossy()
    )));
    let child = Command::new(&exe)
        .arg(script)
        .current_dir(script.parent().unwrap_or(Path::new(".")))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        let _ = tx.send(DeployMsg::Aborted("no se pudo lanzar el script".into()));
        return None;
    };
    let stdout = child.stdout.take();
    let mut events = Vec::new();
    if let Some(out) = stdout {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            match parse_script_line(&line) {
                Some(DeployEvent::Log(text)) => {
                    let _ = tx.send(DeployMsg::Log(text));
                }
                Some(DeployEvent::Error(text)) => {
                    let _ = tx.send(DeployMsg::Log(format!("error del script: {text}")));
                }
                Some(DeployEvent::Skip { path, reason }) => {
                    events.push(DeployEvent::Skip { path, reason });
                }
                Some(other) => events.push(other),
                None => {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        let _ = tx.send(DeployMsg::Log(trimmed.to_string()));
                    }
                }
            }
        }
    }
    let _ = child.wait();
    Some(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site::DEFAULT_INCLUDE;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nuide-deploy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn site() -> Site {
        Site {
            transport: Transport::Ftp,
            host: "ftp.ejemplo.com".into(),
            remote_root: "/www".into(),
            include: DEFAULT_INCLUDE.into(),
            ..Site::default()
        }
    }

    #[test]
    fn collect_filtra_por_extension_y_evita_lo_que_no_es_fuente() {
        let root = tmp("collect");
        std::fs::create_dir_all(root.join("css")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("index.html"), "<h1>h</h1>").unwrap();
        std::fs::write(root.join("css/site.CSS"), b"a{}").unwrap();
        std::fs::write(root.join("secret.txt"), "x").unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::fs::write(root.join("target/nuide.exe"), "x").unwrap();
        std::fs::write(root.join("binario.png.png"), "x").unwrap();

        let mut s = site();
        s.include = "html,css,png".into();
        let found: Vec<String> = collect(&root, &s).into_iter().map(|t| t.remote).collect();
        assert_eq!(found, vec!["binario.png.png", "css/site.CSS", "index.html"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn el_plan_resuelve_rutas_y_cuenta_los_saltos() {
        let root = tmp("plan");
        std::fs::create_dir_all(root.join("js")).unwrap();
        std::fs::write(root.join("js/app.js"), "1").unwrap();
        let events = vec![
            DeployEvent::Upload {
                local: "js/app.js".into(),
                remote: String::new(),
            },
            DeployEvent::Upload {
                local: "js/app.js".into(),
                remote: "public/js/app.min.js".into(),
            },
            DeployEvent::Upload {
                local: "no-existe.js".into(),
                remote: String::new(),
            },
            DeployEvent::Skip {
                path: "grande.wasm".into(),
                reason: "pesado".into(),
            },
            DeployEvent::Log("hola".into()),
        ];
        let (transfers, skipped) = plan(&events, &root, &site());
        assert_eq!(transfers.len(), 2, "{transfers:?}");
        assert_eq!(transfers[0].remote, "www/js/app.js");
        assert_eq!(transfers[1].remote, "www/public/js/app.min.js");
        // El inexistente y el SKIP cuentan como saltados (2).
        assert_eq!(skipped, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn la_transferencia_elige_curl_salvo_para_scp_explicito() {
        let t = Transfer {
            local: PathBuf::from("F:/p/a.html"),
            remote: "www/a.html".into(),
        };
        let s = site();
        let (program, args) = transfer_args(&s, &t);
        assert_eq!(program, CURL);
        assert!(args.join(" ").contains("ftp://ftp.ejemplo.com/www/a.html"));

        let mut scp = site();
        scp.transport = Transport::Sftp;
        scp.cacert = "usa-scp".into();
        let (program, args) = transfer_args(&scp, &t);
        assert_eq!(program, SCP);
        assert!(
            args.join(" ").contains("@ftp.ejemplo.com:www/a.html"),
            "{args:?}"
        );
    }
}
