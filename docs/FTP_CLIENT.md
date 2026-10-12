# Cliente FTP de NuIde (plan)

## Objetivo

Un **cliente FTP/FTPS/SFTP básico** dentro de NuIde, en **pantalla modal** (y
opcionalmente en un panel), que:

1. se conecta a un sitio (reusa el `Site` del proyecto de *Preferencias →
   Despliegue*);
2. **navega** el árbol remoto (entrar/salir de carpetas);
3. **sube / baja / borra / crea carpetas** remotas;
4. puede ser **controlado por NuCoder** (el agente abre/navega/transfiere en la
   UI visible, como ya hace con `run_in_terminal`).

## Por qué encaja sin dependencias nuevas

NuIde ya tiene toda la base de red por **`curl.exe`** (FTP/FTPS/SFTP) y `scp` de
OpenSSH, sin crates:

- `src/site.rs` — modelo `Site` (transporte, host, puerto, user, pass,
  `remote_root`), `Transport` (Ftp/Ftps/Sftp), `url_for`, `curl_args`, `scp_args`.
- `src/deploy.rs` — transferencias en un **hilo** con canal (`DeployMsg`),
  `spawn_test` (ya lista el directorio remoto con `--list-only`).
- Patrón de **módulo opt-in** (`module.sqlite`) + **modal** (`ui/ide_sqlite.slint`,
  `src/sqlite.rs`) — plantilla exacta para el cliente FTP.

## Operaciones (argv de `curl`)

| Operación | Comando |
|---|---|
| Listar | `curl -sS -u user:pass ftp://host/dir/` (devuelve el `LIST` con `d`/tamaño) |
| Descargar | `curl -sS -u user:pass -o <local> ftp://host/dir/file` |
| Subir | `curl -sS --fail -T <local> --ftp-create-dirs … ftp://host/dir/file` (ya en `Site::curl_args`) |
| Borrar | `curl -sS -u … -Q "DELE /dir/file" ftp://host/` |
| Crear carpeta | `curl -sS -u … -Q "MKD /dir/nueva" ftp://host/` |
| Renombrar | `curl -sS -u … -Q "RNFR /a" -Q "RNTO /b" ftp://host/` |

> FTPS valida el certificado por defecto (curl); `cacert` apunta una CA propia.
> Para SFTP se prefiere `curl` (crea carpetas); `scp` es la salida explícita.

## Componentes

### 1) Núcleo: `src/ftp.rs` (nuevo)
- `enum Op { List { path }, Download { remote, local }, Upload { local, remote }, Delete { remote }, Mkdir { remote }, Rename { from, to } }`
- `list_args(site, path)`, `download_args`, `delete_args`, `mkdir_args`, `rename_args`
  (reusan `Site::url_for`/`curl_args`).
- `parse_listing(text) -> Vec<RemoteEntry>` con `RemoteEntry { name, dir, size }`:
  parser tolerante para formato **Unix `ls -l`** y **MS-DOS**; ignora `.`/`..`;
  si no reconoce el formato, guarda las líneas crudas (se muestran tal cual).
- `normalize_remote(path)` (barras `/`, resuelve `..`, normaliza el `remote_root`).
- `spawn_op(site, op, tx) -> JoinHandle<()>` en hilo; `FtpMsg { Listed { path, entries, raw }, Done { op, ok, detail }, Failed(String) }`.
- **Tests**: parseo de ambos formatos, normalización de rutas y argv por operación
  (sin red), en el estilo de `site.rs`/`deploy.rs`.

### 2) UI: `ui/ide_ftp.slint` (nuevo) — modal
- Barra de **conexión**: transporte, host, puerto, usuario, contraseña (oculta),
  botones **Conectar**, **Probar** y **“usar el sitio del proyecto”**.
- **Migas de pan** del path remoto + **↑** (subir un nivel) + **⟳** (refrescar).
- **Lista** de entradas (icono carpeta/archivo, nombre, tamaño). Clic en carpeta
  entra; clic en archivo lo **baja** (con `rfd` para elegir destino; por defecto
  el proyecto).
- Botones: **Subir archivo…** (`rfd`), **Nueva carpeta**, y por fila
  **Bajar**/**Borrar** (con confirmación `IdeAsk`).
- Estado (conectando/ocupado) + nota de error; cierre con `Esc`/clic afuera
  (igual que `IdeSqlite`).

### 3) Integración (`src/editor.rs`, `ui/nuide_ui.slint`)
- Campos en `Ide`: `ftp_enabled`, `ftp_open`, `ftp: FtpClient` (host/puerto/user/
  pass/path/entries/selección/ocupado/nota), `ftp_tx/ftp_rx`, `ftp_gen`.
- **Módulo** `module.ftp` en *Preferencias → Módulos* (fila como SQLite).
- Comando de paleta **“FTP: abrir cliente”** y entrada en el menú **Agente**.
- `drain` procesa `FtpMsg` (con **época**, descarta respuestas viejas) y `sync`
  publica el modelo, como el resto de los canales.
- Perfil por defecto = `Site` del proyecto (un clic para traer host/user/pass).

### 4) Control por NuCoder (host channel, patrón `run_in_terminal`)
- **Permiso nuevo “FTP”** en la pestaña *Permisos* (`permissions.ftp`,
  `Config::perm_ftp`), aplicado en `tools::Ctx`.
- `nucoder::tools`: `FtpJob { op, args, reply }` + herramientas:
  - `ftp_open(path)` — abre el cliente en la IDE y navega (visible para el humano).
  - `ftp_list(path)` — devuelve el listado como texto.
  - `ftp_upload(local, remote)` · `ftp_download(remote, local)`.
  - `ftp_delete(remote)` · `ftp_mkdir(remote)`.
- `Ctx.ftp: Option<&Sender<FtpJob>>` + `ftp_allowed: bool`; el **IDE** atiende los
  `FtpJob` en `drain` (corre `curl` en hilo, actualiza la UI y responde). El
  agente recibe el resultado como texto, igual que con `TerminalJob`.

## Fases

| Fase | Contenido | Tamaño |
|---|---|---|
| **F1** | `src/ftp.rs`: argv por operación + parser de `LIST` + `spawn_op` + tests | S/M |
| **F2** | `ui/ide_ftp.slint` + wiring: abrir/cerrar, listar, navegar, subir/bajar | M |
| **F3** | Borrar / mkdir / renombrar + confirmaciones + ordenar/filtrar | S |
| **F4** | Permiso FTP + `FtpJob` + tools de NuCoder + host en `drain` | M |
| **F5** | Pulido: teclado (↑/↓/Enter/Backspace), doble panel local‖remoto, doc/guía | M |

## Riesgos y decisiones

- **Formato de `LIST`**: varía por servidor (Unix/MDOS); parser tolerante y, si
  falla, mostrar el crudo (nunca romper).
- **`scp` no crea carpetas**: para SFTP preferir `curl` (ya documentado en `site.rs`).
- **Credenciales**: la contraseña vive en el config del usuario
  (`%APPDATA%\NuIde`), nunca en el repo; **no** se loguea ni se muestra.
- **Seguridad de escritura**: subir/borrar remotos es sensible → respeta
  *Confirmación* / *auto-aprobar* como el resto de acciones del agente.
- **Sin dependencias nuevas**: todo con `curl.exe`/`scp` y `std`, como el despliegue.

## Evaluación de crates de FTP (crates.io)

Consulta a la API de crates.io (`q=ftp`). Candidatos y veredicto:

| Crate | Versión / última | Mantenimiento | Notas |
|---|---|---|---|
| **`suppaftp`** | 12.2.0 · 2026-10-11 | activo (veeso) | Cliente FTP/**FTPS** *sync*. 4,85 M descargas (1,67 M recientes). `default-features = []`. FTPS con `native-tls` (en Windows usa **Schannel**, sin OpenSSL). **El candidato**. |
| `ftp` | 3.0.1 · 2018 | muerto | Cliente viejo; sin mantenimiento. **No**. |
| `async_ftp` | 6.0.0 · 2021 | muerto | Tokio/async. **No**. |
| `remotefs-ftp` | 1.0.0 · 2026-09 | activo | Capa fina sobre `ftp`/`suppaftp`; no aporta sobre suppaftp directo. **No** (a menos que se quiera la abstracción). |
| `opendal-service-ftp` | 0.59 · 2026-10 | activo | Parte de Apache OpenDAL (async, pesadísimo). **No**. |
| `ftp-cmd-list-parse` | 0.3.1 · 2021 | quieto | Sólo parsea `LIST`; deps `regex`+`lazy_static`. El parser propio (~80 líneas) es mejor. **No**. |
| `libunftp` | 0.23 · 2026-09 | activo | Es un **servidor** FTP. Irrelevante. **No**. |
| `suppaftp-cli` | 12.2.0 · 2026-10 | activo | CLI basado en suppaftp. Sólo si se prefiere **no** meter el crate y **bundelear** el `.exe` (como `curl`). Alternativa D. |

### Peso real de `suppaftp` (12.2.0)

Dependencias **no opcionales** (siempre entran): `chrono` (clock), `lazy-regex`
(→ `regex` completo), `log`, `thiserror`. Con `features = ["native-tls"]` para
FTPS se suma `native-tls` (+ `schannel` en Windows).

- `default-features` está **vacío**: sin TLS y sin async; se elige por feature.
- Licencia **MIT OR Apache-2.0**. Crate de 15 KB, ~13,7 k líneas.
- **MSRV 1.88 / edición 2024** (requiere toolchain reciente).
- **No hace SFTP** (eso sería `ssh2` → libssh2 en C, o `russh` async).

### Choques con la política actual

1. **"Sin crates de red"**: hoy todo el despliegue va por `curl.exe`/`scp`.
   `suppaftp` rompe esa regla (consciente).
2. **`regex`**: NuIde usa `regex-lite` a propósito para no arrastrar `regex`;
   `suppaftp` lo trae igual (vía `lazy-regex`).
3. **MSRV 1.88 / edición 2024**: hay que confirmar que el toolchain del CI lo
   cumple.
4. **SFTP**: `suppaftp` no lo cubre → seguiría por `curl`/`scp`.

### Recomendación

- **FTP/FTPS → `suppaftp`** (sync encaja con el modelo de hilos de NuIde; maneja
  `MLSD`/`LIST`, `mk`/`rm`/`rename`/resume mucho mejor que parsear `curl`).
  Sugerido: `suppaftp = { version = "12", default-features = false, features =
  ["native-tls"] }`, **detrás de una feature Cargo** (`ftp-client`) para que el
  build por defecto siga liviano y la política quede explícita.
- **SFTP y el pipeline de despliegue → seguir con `curl`/`scp`** (sin cambios).
- Alternativa conservadora (**Opción B**): no agregar crates; `curl` + parser
  `LIST` propio (tolerante). Más frágil en servidores raros, pero cero deps.
- Alternativa **D**: no meter el crate y **bundelear `suppaftp-cli`** como
  herramienta externa (filosofía `curl`), a costa de un binario más en el
  instalador.

> Decisión pendiente: ¿`suppaftp` (FTP/FTPS) + `curl`/`scp` (SFTP), o mantener la
> política sin crates de red? El resto del plan (`src/ftp.rs`, UI, control por
> NuCoder) es igual en ambos casos: sólo cambia el motor de FTP/FTPS.

## Evaluación de la documentación de curl (`everything.curl.dev/ftp`)

Leídas: `index`, `dirlist`, `upload`, `cmds`, `ftps`, `twoconnections`,
`traversedir`. Resumen de lo que aportan al cliente:

| Necesidad | Cómo lo cubre curl | Nota |
|---|---|---|
| Conectar/autenticar | `-u user:pass` o `ftp://user:pass@host` | anónimo por defecto |
| Listar | URL con `/` final → comando **`LIST`** | **no hay formato estándar** (varía por servidor) |
| Listar (solo nombres) | `-l/--list-only` → **`NLST`** | algunos servidores omiten carpetas y enlaces |
| Bajar | `curl -o local ftp://host/dir/file` | — |
| Subir | `-T local ftp://host/dir/` o con ruta completa; `--append`; globbing | ya en `Site::curl_args` |
| Borrar | `-Q "DELE /dir/file"` | comando custom |
| Crear/borrar carpeta | `-Q "MKD /dir"`, `-Q "RMD /dir"` | custom |
| Renombrar | `-Q "RNFR viejo" -Q "RNTO nuevo"` | custom |
| Modo de transferencia | binario por defecto; `-B`/`;type=A` para ASCII | — |
| Pasivo/activo | pasivo por defecto; `-P -` activo; `--ftp-skip-pasv-ip` para PASV roto | — |
| Navegar | `--ftp-method multicwd\|singlecwd\|nocwd` | default `multicwd` |

### Conclusión

**curl cubre el 100 % de las operaciones** del cliente básico (lista, baja, sube,
borra, mkdir/rmdir, renombra) — sin dependencias. El único punto débil real es el
**listado**: no hay formato estándar (`LIST`) y `NLST` pierde las carpetas, así
que hay que **parsear `LIST` con un parser tolerante** (o aceptar `NLST` sin
distinguir tipos). Esto es exactamente lo que resolvería `suppaftp` (hace `MLSD`
y parsea bien).

### Hallazgos que afectan al **despliegue actual** (a corregir)

1. **FTPS**: `Site` arma `ftps://…` (FTPS **implícito**). La doc dice que el
   implícito usa el **puerto 990** por defecto, mientras `Site::default_port()`
   devuelve **21** → un sitio FTPS por defecto termina apuntando al puerto
   equivocado. Lo recomendado es **FTPS explícito**: `ftp://` + `--ssl-reqd`
   (`AUTH TLS`). Conviene agregar/corregir el modo explícito.
2. **Credenciales en `argv`**: `Site::curl_args` pasa `--user user:pass` en la
   línea de comandos → la contraseña queda **visible en la lista de procesos**.
   Mejora: usar `-K <archivo>` (config de curl) o `--netrc`, como ya hace
   `nucoder/src/client.rs` con la API key.
3. **Comandos custom abortan ante error** salvo que se marquen fallibles (`-*`):
   hay que ordenar bien `-Q` (p. ej. `DELE` con `-Q "-*DELE …"`).

## Evaluación: FTP vía PowerShell / `.NET` (la página de powershellcommands)

La página propone **`.NET` desde PowerShell**: `System.Net.WebClient`
(`UploadFile`/`DownloadFile`) y `System.Net.FtpWebRequest`
(`ListDirectory`, `DeleteFile`, `MakeDirectory`, `RemoveDirectory`, `Rename`,
`EnableSsl`). Ventaja: **cero crates y cero binarios extra** (viene con
Windows).

### Verificado en esta máquina

```
powershell.exe (5.1) → [System.Net.FtpWebRequest] = System.Net.FtpWebRequest   ✅ existe
pwsh (7)             → no instalado; y en .NET Core/.NET 5+ la clase NO existe
curl.exe             → C:\WINDOWS\system32\curl.exe                            ✅
```

**Los hechos que deciden:**

1. **`FtpWebRequest` es de .NET Framework, no de .NET Core.** En **PowerShell 7
   (`pwsh`)** la clase **no existe** y `WebClient` perdió el soporte FTP. Es decir,
   este camino **sólo funciona con Windows PowerShell 5.1** (`powershell.exe`).
   NuIde usa `pwsh` como shell preferido → habría que **forzar 5.1** sólo para FTP.
2. **API legacy/deprecada**: Microsoft marca `FtpWebRequest`/`WebClient` como
   obsoletos (no recomendados para código nuevo).
3. **Windows-only**: el CI corre en Linux; los tests de FTP tendrían que saltearse.
4. **Contraseña**: se evita el `argv` usando `NetworkCredential`, pero hay que
   pasarla al proceso 5.1 con cuidado (script temporal o variable de entorno).
5. **Una ventaja real**: `EnableSsl = $true` hace **FTPS explícito** (AUTH TLS)
   de forma nativa y ordenada.

### Conclusión sobre esto

**No cambia la decisión.** El FTPS explícito —el único punto donde `.NET`
aporta— **curl también lo hace**: `ftp://` + `--ssl-reqd` (o `--ssl`) manda
`AUTH TLS` en el puerto 21. Así que **no hace falta un segundo motor
Windows-only y legacy**: el cliente se resuelve con curl y, si algún servidor se
porta raro, `powershell.exe` (5.1) con `FtpWebRequest` queda como **fallback
opcional documentado**, no como base.

> Si se quisiera **cero curl** en Windows, la alternativa sería PowerShell 5.1 +
> `FtpWebRequest` como motor único; pero eso ata el cliente a Windows + 5.1 y
> rompe el CI en Linux. No recomendado.

## Estado del plan (implementado)

- **Motor (F1)**: `src/ftp.rs` con `Op`, parser de `LIST` (Unix + MS-DOS),
  credenciales por `-K` y `spawn_op`/`run_job`, con tests.
- **Modal (F2/F3)**: `ui/ide_ftp.slint` + módulo `module.ftp` en Preferencias →
  Módulos; listar/navegar/subir/bajar/borrar/mkdir.
- **Control por NuCoder (F4)**: permiso **FTP** + tools `ftp_*` + `FtpJob`; el
  IDE ejecuta con `curl` y responde al agente.
- **Decisiones aplicadas**: motor **curl** (Opción B); **FTPS explícito**
  (`ftp://` + `--ssl-reqd`, puerto 21; implícito sólo en 990) y **credenciales
  por `-K`** (no en `argv`).

## Puntos de código existentes a reusar

- `src/site.rs` (modelo + argv de curl/scp) — extender con las ops nuevas.
- `src/deploy.rs` (hilo + canal + `spawn_test`) — mismo patrón.
- `src/sqlite.rs` + `ui/ide_sqlite.slint` — plantilla módulo/modal.
- `src/editor.rs`: struct (~1269), `new` (~1407), módulos (~4310/4826),
  `drain` (~8042), `sync` (~7678), comandos de paleta.
- `nucoder/src/tools.rs` + `agent.rs` — patrón `TerminalJob` / `permissions`.
- `ui/ide_nucoder.slint` — nueva fila de permiso “FTP”.
