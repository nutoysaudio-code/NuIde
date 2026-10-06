//! Sitio de despliegue del proyecto (FTP/FTPS por `curl`, SFTP/SSH por OpenSSH).
//!
//! Sin crates de red a propósito: Windows ya trae `curl.exe` (que habla `ftp`,
//! `ftps` y `sftp`) y el cliente OpenSSH (`ssh`, `scp`, `sftp`). Agregar
//! `suppaftp`/`russh`/`ssh2` al build no paga el costo.
//!
//! **La lógica de *qué* subir vive en un script SpASM del proyecto** (por defecto
//! `deploy.spasm`, ejecutado con la toolchain que ya detecta [`crate::spasm`]).
//! NuIde corre el script y le lee stdout línea por línea con un contrato simple,
//! que es el mismo estilo que usa el host de SpASM (`popen` + líneas):
//!
//! ```text
//! UPLOAD <ruta-local-relativa> [<ruta-remota-relativa>]
//! SKIP   <ruta-local-relativa> [motivo]
//! LOG    <texto>
//! ERROR  <texto>
//! ```
//!
//! NuIde es el que **ejecuta la transferencia** (curl/scp), así el script no
//! necesita un builtin para lanzar procesos. Cada línea se traduce a un
//! [`DeployEvent`] que la UI muestra en la sesión `build` de la terminal.
//!
//! Los datos del sitio se guardan en el config de NuIde **indexados por
//! proyecto** (`site.<ruta>.host`), de dos maneras: no se pisan entre proyectos y
//! nunca tocan el repositorio del proyecto (donde podrían comitearse).

use std::path::{Path, PathBuf};

use crate::config::Config;

/// Extensiones que se suben por defecto (sitio web estático).
pub const DEFAULT_INCLUDE: &str =
    "html,htm,css,js,mjs,json,txt,xml,php,md,png,jpg,jpeg,gif,svg,webp,ico,wasm";

/// Script de despliegue buscado en la raíz del proyecto.
pub const DEFAULT_SCRIPT: &str = "deploy.spasm";

/// Protocolo de transferencia del sitio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// FTP sin cifrar (puerto 21).
    Ftp,
    /// FTP sobre TLS (`ftps://`, 21 con AUTH TLS; curl valida el certificado).
    Ftps,
    /// SFTP sobre SSH (22), con `curl` o con `scp` de OpenSSH.
    Sftp,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Ftp => "FTP",
            Transport::Ftps => "FTPS",
            Transport::Sftp => "SFTP",
        }
    }

    pub fn from_label(s: &str) -> Transport {
        match s.trim().to_ascii_lowercase().as_str() {
            "ftps" | "ftp+tls" | "ftp-tls" => Transport::Ftps,
            "sftp" | "scp" | "ssh" => Transport::Sftp,
            _ => Transport::Ftp,
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Transport::Ftp | Transport::Ftps => 21,
            Transport::Sftp => 22,
        }
    }

    /// Esquema que usa curl para este transporte.
    pub fn scheme(self) -> &'static str {
        match self {
            Transport::Ftp => "ftp",
            Transport::Ftps => "ftps",
            Transport::Sftp => "sftp",
        }
    }
}

/// Un sitio de despliegue asociado a un proyecto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub transport: Transport,
    pub host: String,
    /// 0 = el puerto por defecto del transporte.
    pub port: u16,
    pub user: String,
    /// En texto plano en el config del usuario (decisión tomada a conciencia:
    /// el config vive en `%APPDATA%\NuIde`, no en el repositorio).
    pub pass: String,
    /// Carpeta remota donde cae el sitio (`/www/mi-sitio`, `public_html`, ...).
    pub remote_root: String,
    /// Subcarpeta local que se despliega ("" = la raíz del proyecto).
    pub local_sub: String,
    /// Lista de extensiones separadas por coma.
    pub include: String,
    /// Script SpASM que decide qué subir (vacío = subir todo el filtro).
    pub script: String,
    /// PEM con la CA para validar el certificado en FTPS (vacío = almacén del
    /// sistema, que es lo que hace curl por defecto).
    pub cacert: String,
}

impl Default for Site {
    fn default() -> Self {
        Self {
            transport: Transport::Ftp,
            host: String::new(),
            port: 0,
            user: String::new(),
            pass: String::new(),
            remote_root: String::new(),
            local_sub: String::new(),
            include: DEFAULT_INCLUDE.to_string(),
            script: String::new(),
            cacert: String::new(),
        }
    }
}

/// Clave del config para un campo del sitio de `project`.
fn key(project: &Path, field: &str) -> String {
    format!("site.{}.{field}", project.display())
}

impl Site {
    /// Carga el sitio del proyecto (valores por defecto si no hay nada).
    pub fn load(project: &Path, cfg: &Config) -> Site {
        let get = |field: &str, def: &str| {
            cfg.get(&key(project, field))
                .map(str::to_string)
                .unwrap_or_else(|| def.to_string())
        };
        let port = cfg
            .get(&key(project, "port"))
            .and_then(|s| s.trim().parse::<u16>().ok())
            .unwrap_or(0);
        Site {
            transport: Transport::from_label(&get("transport", "ftp")),
            host: get("host", ""),
            port,
            user: get("user", ""),
            pass: get("pass", ""),
            remote_root: get("remote", ""),
            local_sub: get("local-sub", ""),
            include: get("include", DEFAULT_INCLUDE),
            script: get("script", ""),
            cacert: get("cacert", ""),
        }
    }

    /// Persiste el sitio del proyecto.
    pub fn save(&self, project: &Path, cfg: &mut Config) {
        let mut put = |field: &str, value: String| {
            let k = key(project, field);
            cfg.set(&k, value);
        };
        put("transport", self.transport.label().to_string());
        put("host", self.host.clone());
        put("port", self.port.to_string());
        put("user", self.user.clone());
        put("pass", self.pass.clone());
        put("remote", self.remote_root.clone());
        put("local-sub", self.local_sub.clone());
        put("include", self.include.clone());
        put("script", self.script.clone());
        put("cacert", self.cacert.clone());
    }

    /// Valor actual de un campo editable del sitio (para el diálogo de entrada).
    pub fn field(&self, field: &str) -> &str {
        match field {
            "user" => &self.user,
            "pass" => &self.pass,
            "remote" => &self.remote_root,
            "local-sub" => &self.local_sub,
            "include" => &self.include,
            "script" => &self.script,
            "cacert" => &self.cacert,
            _ => &self.host,
        }
    }

    /// `true` si hay un host cargado (el menú puede ofrecer "Subir cambios").
    pub fn is_configured(&self) -> bool {
        !self.host.trim().is_empty()
    }

    pub fn port_or_default(&self) -> u16 {
        if self.port == 0 {
            self.transport.default_port()
        } else {
            self.port
        }
    }

    /// Extensiones en minúscula, sin punto.
    pub fn extensions(&self) -> Vec<String> {
        self.include
            .split(',')
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Raíz local que se despliega.
    pub fn local_root(&self, project: &Path) -> PathBuf {
        if self.local_sub.trim().is_empty() {
            project.to_path_buf()
        } else {
            project.join(self.local_sub.trim_matches(|c| c == '/' || c == '\\'))
        }
    }

    /// Ruta del script de despliegue, si el proyecto lo tiene.
    pub fn script_path(&self, project: &Path) -> Option<PathBuf> {
        let explicit = !self.script.trim().is_empty();
        let candidate = if explicit {
            PathBuf::from(self.script.trim())
        } else {
            project.join(DEFAULT_SCRIPT)
        };
        let full = if candidate.is_absolute() {
            candidate
        } else {
            project.join(candidate)
        };
        full.is_file().then_some(full)
    }

    /// Ruta remota para una ruta relativa a `local_root`, con barras al estilo
    /// URL (`public_html/css/site.css`).
    pub fn remote_for(&self, rel: &Path) -> String {
        let sub = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>()
            .join("/");
        let base = self.remote_root.trim().replace('\\', "/");
        let base = base.trim_matches('/');
        if base.is_empty() {
            sub
        } else if sub.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{sub}")
        }
    }

    /// URL de destino para curl (`ftps://host/ruta`).
    pub fn url_for(&self, remote: &str) -> String {
        let port = self.port_or_default();
        let suffix = if port == self.transport.default_port() {
            String::new()
        } else {
            format!(":{port}")
        };
        let remote = remote.replace('\\', "/");
        let remote = if remote.starts_with('/') {
            remote
        } else {
            format!("/{remote}")
        };
        format!(
            "{}://{}{}{}",
            self.transport.scheme(),
            self.host,
            suffix,
            remote
        )
    }

    /// Argumentos de `curl` para subir un archivo.
    ///
    /// `-T` sube (STOR/PUT), `--ftp-create-dirs` crea las carpetas del camino,
    /// `-sS` silencio el progreso pero deja los errores, y `--fail` hace que un
    /// error de transferencia sea código no-cero. En FTPS curl **valida el
    /// certificado por defecto**; `cacert` apunta una CA propia si el host usa
    /// un certificado interno.
    pub fn curl_args(&self, local: &Path, remote: &str) -> Vec<String> {
        let mut args = vec![
            "-sS".to_string(),
            "--fail".to_string(),
            "-T".to_string(),
            local.display().to_string(),
        ];
        if self.transport == Transport::Ftp || self.transport == Transport::Ftps {
            args.push("--ftp-create-dirs".to_string());
        }
        if self.transport == Transport::Ftps && !self.cacert.trim().is_empty() {
            args.push("--cacert".to_string());
            args.push(self.cacert.trim().to_string());
        }
        if !self.user.is_empty() {
            args.push("--user".to_string());
            args.push(format!("{}:{}", self.user, self.pass));
        }
        args.push(self.url_for(remote));
        args
    }

    /// Argumentos de `scp` (SFTP con el cliente de OpenSSH) para subir un
    /// archivo. `remote` llega **ya resuelto** (igual que en `curl_args`): aplicar
    /// otra vez `remote_root` duplicaría el prefijo. Nota: `scp` no crea las
    /// carpetas del camino, por eso el transporte SFTP prefiere `curl`.
    pub fn scp_args(&self, local: &Path, remote: &str) -> Vec<String> {
        let mut args = vec![
            "-q".to_string(),
            "-P".to_string(),
            self.port_or_default().to_string(),
        ];
        if !self.cacert.trim().is_empty() && self.cacert.trim() != "usa-scp" {
            // En SSH el equivalente de "esta CA" es la clave del host: se acepta
            // la primera vez y queda en `known_hosts`.
            args.push("-o".to_string());
            args.push(format!("UserKnownHostsFile={}", self.cacert.trim()));
        }
        args.push(local.display().to_string());
        let remote = remote.replace('\\', "/");
        let remote = remote.trim_start_matches('/');
        args.push(format!("{}@{}:{remote}", self.user, self.host));
        args
    }

    /// Destino para `ssh` (la terminal remota del panel).
    pub fn ssh_target(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// Argumentos para abrir una terminal SSH interactiva.
    pub fn ssh_args(&self) -> Vec<String> {
        vec![
            "-t".to_string(),
            "-p".to_string(),
            self.port_or_default().to_string(),
            self.ssh_target(),
        ]
    }
}

/// Qué pidió el script de despliegue (o qué pasó al ejecutarlo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployEvent {
    /// El script pidió subir un archivo. `remote` vacío = la ruta por defecto.
    Upload { local: String, remote: String },
    /// El script saltó un archivo (con motivo opcional).
    Skip { path: String, reason: String },
    /// Línea de información del script.
    Log(String),
    /// Error reportado por el script.
    Error(String),
    /// NuIde subió un archivo (resultado de ejecutar curl/scp).
    Sent { path: String, ok: bool },
    /// Terminó la corrida: (subidos, saltados, fallados).
    Done {
        sent: usize,
        skipped: usize,
        failed: usize,
    },
}

/// Interpreta una línea del script. `None` si no es del protocolo (se muestra
/// como texto suelto).
pub fn parse_script_line(line: &str) -> Option<DeployEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // Se acepta el prefijo ">" opcional, para que el script pueda mezclar salida
    // propia con instrucciones sin ambigüedad.
    let body = line.strip_prefix('>').unwrap_or(line);
    let mut it = body.splitn(3, [' ', '\t']);
    let verb = it.next()?.to_ascii_uppercase();
    match verb.as_str() {
        "UPLOAD" | "PUT" => {
            let local = it.next()?.to_string();
            if local.is_empty() {
                return None;
            }
            Some(DeployEvent::Upload {
                local,
                remote: it.next().unwrap_or("").to_string(),
            })
        }
        "SKIP" => {
            let path = it.next()?.to_string();
            Some(DeployEvent::Skip {
                path,
                reason: it.next().unwrap_or("").to_string(),
            })
        }
        "LOG" | "INFO" => Some(DeployEvent::Log(body[verb.len()..].trim().to_string())),
        "ERROR" | "ERR" => Some(DeployEvent::Error(body[verb.len()..].trim().to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(transport: Transport) -> Site {
        Site {
            transport,
            host: "ftp.ejemplo.com".into(),
            port: 0,
            user: "nutoys".into(),
            pass: "secreto".into(),
            remote_root: "/public_html".into(),
            local_sub: String::new(),
            include: DEFAULT_INCLUDE.into(),
            script: String::new(),
            cacert: String::new(),
        }
    }

    #[test]
    fn las_rutas_remotas_usan_barras_y_el_root_del_sitio() {
        let s = site(Transport::Ftp);
        assert_eq!(
            s.remote_for(Path::new("css/site.css")),
            "public_html/css/site.css"
        );
        // Raíz vacía = mismo camino relativo.
        let mut sin_root = s.clone();
        sin_root.remote_root = String::new();
        assert_eq!(sin_root.remote_for(Path::new("a\\b.html")), "a/b.html");
        // Un subdirectorio local no se duplica en el remoto.
        let mut sub = s.clone();
        sub.local_sub = "build".into();
        assert_eq!(
            sub.local_root(Path::new("F:/proy"))
                .to_string_lossy()
                .replace('\\', "/"),
            "F:/proy/build"
        );
    }

    #[test]
    fn las_urls_siguen_el_transporte_y_el_puerto_solo_si_es_distinto() {
        assert_eq!(
            site(Transport::Ftp).url_for("public_html/a.html"),
            "ftp://ftp.ejemplo.com/public_html/a.html"
        );
        assert_eq!(
            site(Transport::Ftps).url_for("/x.css"),
            "ftps://ftp.ejemplo.com/x.css"
        );
        assert_eq!(
            site(Transport::Sftp).url_for("x"),
            "sftp://ftp.ejemplo.com/x"
        );
        let mut otro = site(Transport::Ftp);
        otro.port = 2121;
        assert_eq!(otro.url_for("y"), "ftp://ftp.ejemplo.com:2121/y");
    }

    #[test]
    fn curl_args_sube_el_archivo_crea_carpetas_y_valida_ftps() {
        let args = site(Transport::Ftps).curl_args(Path::new("F:/p/a.html"), "public_html/a.html");
        let joined = args.join(" ");
        assert!(joined.contains("-T"), "{joined}");
        assert!(joined.contains("--ftp-create-dirs"), "{joined}");
        assert!(joined.contains("--fail"), "{joined}");
        assert!(
            joined.contains("ftps://ftp.ejemplo.com/public_html/a.html"),
            "{joined}"
        );
        // FTP plano no necesita `--cacert`; FTPS con CA propia sí.
        assert!(!joined.contains("--cacert"), "{joined}");
        let mut con_ca = site(Transport::Ftps);
        con_ca.cacert = "F:/certs/mi-ca.pem".into();
        assert!(con_ca
            .curl_args(Path::new("a"), "b")
            .join(" ")
            .contains("--cacert"));
        // SFTP: no se agrega la opción específica de FTP.
        let sftp = site(Transport::Sftp)
            .curl_args(Path::new("a"), "b")
            .join(" ");
        assert!(!sftp.contains("--ftp-create-dirs"), "{sftp}");
    }

    #[test]
    fn el_protocolo_del_script_acepta_verbos_y_prefijo() {
        assert_eq!(
            parse_script_line("UPLOAD css/site.css"),
            Some(DeployEvent::Upload {
                local: "css/site.css".into(),
                remote: String::new()
            })
        );
        assert_eq!(
            parse_script_line(">put index.html public_html/index.html"),
            Some(DeployEvent::Upload {
                local: "index.html".into(),
                remote: "public_html/index.html".into()
            })
        );
        assert_eq!(
            parse_script_line("SKIP build/grande.wasm pesado"),
            Some(DeployEvent::Skip {
                path: "build/grande.wasm".into(),
                reason: "pesado".into()
            })
        );
        assert_eq!(
            parse_script_line("LOG 3 archivos"),
            Some(DeployEvent::Log("3 archivos".into()))
        );
        assert_eq!(
            parse_script_line("ERROR sin permiso"),
            Some(DeployEvent::Error("sin permiso".into()))
        );
        // Lo que no es protocolo no se interpreta (el script puede imprimir lo
        // que quiera sin romper nada).
        assert_eq!(parse_script_line("hola mundo"), None);
        assert_eq!(parse_script_line("   "), None);
    }

    #[test]
    fn el_filtro_de_extensiones_se_normaliza() {
        let mut s = site(Transport::Ftp);
        s.include = " .HTML , css ,, js ".into();
        assert_eq!(s.extensions(), vec!["html", "css", "js"]);
    }

    #[test]
    fn las_teclas_del_sitio_van_indexadas_por_proyecto() {
        assert_eq!(key(Path::new("F:/proy"), "host"), "site.F:/proy.host");
        // Dos proyectos no se pisan entre sí.
        assert_ne!(
            key(Path::new("F:/a"), "host"),
            key(Path::new("F:/b"), "host")
        );
    }

    #[test]
    fn el_transporte_tiene_etiqueta_puerto_y_esquema() {
        for t in [Transport::Ftp, Transport::Ftps, Transport::Sftp] {
            assert_eq!(Transport::from_label(t.label()), t);
            assert!(!t.scheme().is_empty());
            assert!(t.default_port() > 0);
        }
        assert_eq!(Transport::from_label("scp"), Transport::Sftp);
        assert_eq!(Transport::from_label(""), Transport::Ftp);
    }

    #[test]
    fn field_devuelve_el_valor_de_cada_campo_editable() {
        let s = site(Transport::Sftp);
        assert_eq!(s.field("host"), "ftp.ejemplo.com");
        assert_eq!(s.field("user"), "nutoys");
        assert_eq!(s.field("pass"), "secreto");
        assert_eq!(s.field("remote"), "/public_html");
        assert_eq!(s.field("include"), DEFAULT_INCLUDE);
        // Un campo desconocido no rompe: cae en host (el diálogo siempre muestra
        // algo editable).
        assert_eq!(s.field("no-existe"), "ftp.ejemplo.com");
    }

    #[test]
    fn ssh_args_apuntan_a_la_terminal_interactiva() {
        let mut s = site(Transport::Sftp);
        s.port = 22022;
        assert_eq!(s.ssh_target(), "nutoys@ftp.ejemplo.com");
        let args = s.ssh_args();
        assert_eq!(args, vec!["-t", "-p", "22022", "nutoys@ftp.ejemplo.com"]);
    }
}
