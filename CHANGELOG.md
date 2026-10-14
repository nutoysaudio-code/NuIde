# Changelog

Todos los cambios relevantes de NuIde. El formato sigue
[Keep a Changelog](https://keepachangelog.com/es/1.1.0/) y el proyecto usa
versionado semántico.

## [0.3.7] — 2026-10-13

### Cambiado

- **Los ajustes de NuIde también van a SQLite**: `src/config.rs` usa la tabla
  `ide_settings` de la misma base (`~/.config/nucoder/nucoder.db`). La primera vez
  **migra** desde `%APPDATA%\NuIde\config` y el archivo queda como **export**. Así
  config, chats, recetario y ajustes de la IDE viven en una sola base.

## [0.3.6] — 2026-10-13

### Cambiado

- **Visor de SQLite nativo**: `src/sqlite.rs` ya no llama a `sqlite3.exe`/`python`
  y les lee TSV — lee la base **directo** con `rusqlite` (misma API pública, el
  panel no cambió). Funciona sin depender de binarios externos. `nuide` suma
  `rusqlite` (misma versión que `nucoder`). Tests del visor reescritos + uno que
  crea y lee una base real.

## [0.3.5] — 2026-10-13

### Agregado

- **Persistencia en SQLite**: NuCoder guarda todo en `~/.config/nucoder/nucoder.db`
  (SQLite, vía `rusqlite/bundled`): la **config** (`settings['config']`), los
  **chats** (`sessions`, uno por proyecto) y el **recetario** de NuCook (`cookbook`).
  La primera vez **migra** desde `config.json`, `sessions/*.json` y
  `cookbook/*.cookie` (los archivos **no** se borran: quedan de respaldo).
- Módulo `nucoder/src/db.rs` (apertura + esquema + helpers) con tests.
- `config.json` se sigue leyendo si existe (y se reimporta) y se reescribe como
  **exportación** legible/editable, pero la fuente de verdad es la base.

### Cambiado

- `nucoder/src/session.rs` y `cook.rs` (recetario) ahora leen/escriben la base.
- **Primera dependencia pesada**: `rusqlite` con `bundled` (compila SQLite en C).
  Se justifica: era el único camino serio para guardar en SQLite sin implementar
  el formato a mano.

## [0.3.4] — 2026-10-13

### Agregado

- **Gestor de skills** (modal *Skills*): fila de acciones nueva con **Actualizar**
  (re-instala desde el origen que se recuerda al instalar), **Permiso** (cicla
  allow → ask → deny por skill, persistido) y **Catálogo…** (URL del `index.json`
  para el buscador web, persistida). El **origen** y la **versión** del skill se
  muestran en el detalle, y la versión también en la lista.

### Cambiado

- `nucoder/src/hub.rs`: al instalar se recuerda el origen (`.nucook-origin`);
  nuevo `hub::update` que re-instala desde ahí.
- El buscador web usa la URL de catálogo configurada en el panel
  (`nucoder.catalog-url`), con `NUCOOK_CATALOG` como respaldo (antes sólo esa
  variable).

## [0.3.3] — 2026-10-13

### Agregado

- **Listado de modelos: filtros «solo gratis» y «solo ok»** en la pestaña modelos
  (`solo ok` muestra únicamente los que respondieron al ping; `solo gratis` filtra
  el listado además del ruteo) y botón **limpiar** para borrar el estado aprendido.
- **Estado aprendido de modelos persistido**: los modelos que fallan (404, timeout,
  error del proveedor, respuesta vacía) quedan marcados como «no funciona» y el
  **router los saltea**; los que responden al ping quedan como OK. Todo vive en la
  config de NuCoder (`model_dead` / `model_ok`).
- **Gratis por defecto**: el router arranca considerando sólo modelos gratuitos
  (la chip «gratis» ahora también **persiste** y filtra el listado).

### Corregido

- **NuCoder ya no se cuelga** cuando un modelo no responde: el chat de `curl` lleva
  un **watchdog de inactividad** (60 s sin bytes ⇒ aborta) y el agente **rota** a
  otro modelo en lugar de esperar hasta el `max-time` (600 s). También rota ante
  errores de red/parseo y trata una respuesta **vacía** (200 sin texto ni
  herramientas) como fallo, en vez de cerrar el turno en silencio.
- **`SKILL.md` con *block scalars* de YAML** (`description: >` / `|`): el parser de
  *frontmatter* de NuCoder no los entendía y dejaba la descripción como `>`. Ahora
  pliega (`>`) o conserva los saltos (`|`), así los skills que usan descripciones
  multilínea (p. ej. `caveman`, `ponytail`) se anuncian bien.

### Cambiado

- **Barra de NuCoder con íconos**: los ítems del encabezado y de la barra
  (modelos, permisos, tarea, imagen, contexto, gratis, auto-aprobar, nueva,
  orden, probar, sincronizar, solo ok, limpiar) pasan a **botones de ícono** con
  **tooltip** de ayuda, para que entren en paneles angostos (el estado —activo,
  imágenes adjuntas— se ve por el resaltado y el contador).

## [0.3.2] — 2026-10-12

### Agregado

- **NuCoder — panel nativo en NuIde (Slint)**: el núcleo pasó a ser una
  **librería UI-agnóstica**; el chat vive en el panel del agente (tarjetas con
  bordes y **avatares**, Markdown + resaltado de código, scroll con *stick*).
  Barra con **modelo / tarea / contexto / imagen**, y switch de **auto-aprobar**.
- **Pestaña Permisos**: Internet, leer fuera del proyecto, terminales de la IDE,
  **cliente FTP**, sandbox de escrituras, confirmación y **autopiloto**.
- **Pestaña Modelos**: listado de **todos** los modelos con **especialidad**
  (coder/visión/imagen/voz/video/…), **filtro**, **orden** (especialidad, ping,
  nombre, gratis), **probar/ping** («probar todos») y **sincronizar** el catálogo.
- **Herramientas del agente**: `run_in_terminal` (comandos en terminales reales
  de la IDE), **cliente FTP** (`ftp_open`/`ftp_list`/`ftp_upload`/`ftp_download`/
  `ftp_delete`/`ftp_mkdir`), `image_generate` y `audio_transcribe`.
- **Plan + autopiloto**: `submit_plan` presenta un plan que se aprueba; con
  autopiloto el agente sigue hasta terminarlo sin pedir permiso.
- **Completado semántico de SpASM** en el editor (`Ctrl+Espacio`) con el AST real
  (símbolos + palabras clave).
- **CLI**: `nucoder refresh [id] [--reset-free]` y `nucoder config <clave> <valor>`.
- **Cliente FTP** (modal, módulo opt-in): conectar (o traer el sitio del proyecto),
  navegar, subir/bajar/borrar, nueva carpeta y filtro del listado.

### Cambiado

- La **TUI (ratatui)** de NuCoder se reemplazó por el panel nativo; el binario
  `nucoder` queda como **CLI**.
- **Sin ventanas de consola**: los procesos hijos (`curl`, `git`, `spasmc`, `cc`,
  `sqlite3`, el script de despliegue) corren ocultos (NuIde y NuCoder).
- **FTPS explícito** (`ftp://` + `--ssl-reqd`, puerto 21; implícito sólo en 990) y
  **credenciales por `curl -K`** (no en la línea de comandos).
- **Ejecutar/Compilar** avisan y revelan la terminal cuando falta archivo o
  toolchain.

### Seguridad

- Las **API keys** viven fuera del repo (`~/.config/nucoder/config.json`); **test
  guardián** que falla si una key queda versionada + bloque de `.gitignore`.

### Eliminado

- `nucoder/src/tui.rs` y `nucoder/src/theme.rs` (TUI reemplazada por el panel).

## [0.3.1] — 2026-10-11

### Agregado

- **Presentación de NuCoder**: al iniciar la TUI (y en `--help`) muestra arte
  ASCII moderno con la versión; si el panel es angosto se usa un título compacto.

## [0.3.0] — 2026-10-11

### Agregado

- **Menú contextual del coder**: clic derecho en el panel del agente suma los
  atajos de los comandos de NuCoder (tareas se insertan, acciones se ejecutan).
- **NuCoder — proveedores por preset** (`provider add openrouter --api-key …`) y
  **filtro de gratis por proveedor** (`free_filter`: auto/pricing/name/all/list).
- **NuCoder — capacidades por modelo** (text/code/vision/image/audio/video/…),
  listado/filtro (`nucoder models --free --cap image`) y ruteo (`--cap`).
- **NuCoder — imágenes (visión)**: `/img` y `--img` adjuntan una imagen; el turno
  se rutea a un modelo con visión.
- **NuCoder — modelo forzado** (`--model`, `/model [n|id|auto]`, con Tab).
- **NuCoder — chatear con Markdown** (títulos, listas, tablas, enlaces) con
  **resaltado de sintaxis**, y paleta alineada al tema del IDE.
- **NuCoder — memoria/contexto**: tope configurable (`context_chars`),
  compactación, `/context`, y **sesiones por proyecto** (`/save`, `/load`,
  `/sessions`, auto al salir/entrar).
- **NuCoder — internet** (`web_fetch`, `web_search`), **KB local** (`read_docs`),
  **sandbox** y **confirmación** de acciones sensibles.

### Cambiado

- **IDE — hilos y estados**: épocas (`gen`) en build/git/sqlite/lint/explorer para
  descartar resultados viejos, tope de drenado por tick en los PTY, cancelación
  cooperativa del build y **explorador reconstruido fuera del hilo de UI**.

## [0.2.2] — 2026-10-08

### Agregado

- **Proveedores de OpenCode (CRUD).** Nueva sección **Proveedores** en la
  configuración: lista los proveedores del CLI y permite **editar, crear y
  eliminar**. Trabaja sobre la config global
  (`~/.config/opencode/opencode.json(c)`) como la **unión** de `opencode.json`
  y `opencode.jsonc`: al guardar escribe todo en `opencode.json` (lo crea si
  falta) y retira `providers` del `.jsonc` para no dejar copias. La escritura es
  **quirúrgica** (sólo se reemplaza el fragmento `providers`), así que se
  conservan `$schema`, `model` y las claves desconocidas de cada proveedor
  (`env`, `modelID`, `limit`, `cost`, …). Los **modelos** se editan como JSON
  crudo y se validan al guardar; eliminar pide confirmación.
- **Minimizar / Maximizar** la ventana desde el menú de la barra superior.
- `src/jsonx.rs`: utilidades JSON con *spans* (tolerantes a comentarios JSONC) y
  validador, base del CRUD.
- **Menú contextual en las terminales** (clic derecho): **Copiar**, **Pegar** y
  **Elevar a administrador**. Vale para la shell y para el panel de OpenCode CLI.
- **Elevación (Windows):** la paleta suma *Terminal: nueva como administrador*
  (shell elevada en ventana aparte, vía UAC) y *NuIde: reiniciar como
  administrador* (relanza toda la IDE elevada). El pie muestra un distintivo
  **ADMIN** cuando la app corre elevada.

### Cambiado

- **Menú superior:** se quita el botón de «Ver» duplicado (hamburguesa); el menú
  queda sólo en la barra de menús y el ítem **Ver** describe el dock de cada
  panel.
- **Ayuda → Ver repositorio** abre el **remoto real** del proyecto
  (`git remote get-url origin`), con el repositorio de NuIde como respaldo. Antes
  apuntaba a GitLab y daba 403.
- **Renderer por defecto:** **FemtoVG** en vez del software, porque el software
  se transparentaba al maximizar. Se puede volver con
  `SLINT_BACKEND=winit-software`.
- **Terminales:** `vt.rs` usa un hash FNV-1a y un flag *dirty*; si no hay salida
  nueva ni cambió la selección, no recalcula filas.
- **Acerca de:** rediseño del diálogo (créditos y versión).
- Título de la ventana `NuIde` y divisores transparentes.

### Corregido

- CI (`.gitlab-ci.yml`) en verde: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings` y `cargo test` (159 pruebas OK).
