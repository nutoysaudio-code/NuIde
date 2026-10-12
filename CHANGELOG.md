# Changelog

Todos los cambios relevantes de NuIde. El formato sigue
[Keep a Changelog](https://keepachangelog.com/es/1.1.0/) y el proyecto usa
versionado semántico.

## [0.3.2] — 2026-10-11

### Corregido

- **NuCoder**: ya no captura el mouse. Antes activaba el *mouse reporting* pero no
  procesaba eventos de mouse, así que la terminal exigía `Shift` para seleccionar
  texto y no se podía copiar. Ahora la selección/copia nativa funciona en el panel
  del agente (el scroll sigue por teclado).

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
