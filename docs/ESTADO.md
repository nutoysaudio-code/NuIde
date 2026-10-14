# NuIde — estado y pendientes

**Actualizado:** 2026-10-13.
Este es el *parte de estado*: qué está hecho, qué se cambió en la última
iteración, qué limitaciones quedan y qué falta. El [README](../README.md) es la
guía de uso; la especificación visual/funcional es
[`NUIDE_IDEA.md`](NUIDE_IDEA.md).

> **Novedades (2026-10-13)**: el agente suma **skills** de Claude/OpenCode/
> nativos/`.agents` (con permisos y `@id`), **NuCook** (cocina un skill en su
> forma enseñable —determinista + gourmet cacheado— con **recetario**, CLI
> **`nucook`** y cocina de docs/KB/datasets), **cliente MCP** (REA: 170 tools,
> conexión persistente) y el **catálogo de skills en la IDE** (buscar local/web,
> instalar, cocinar, eliminar). Ver §3.52–3.59.

---

## 1. Qué es NuIde

IDE de escritorio con **soporte de SpASM** y de otros lenguajes (Rust + Slint):
explorador de archivos, editor con resaltado para ~24 lenguajes,
**dos terminales reales** (una shell local y la TUI de OpenCode) y
barra de estado. Proyecto independiente, sin workspace ni crates compartidos del
ecosistema de audio.

Dependencias: `slint` (UI), `arboard` (portapapeles), `portable-pty` + `vt100`
(terminales) y `rfd` (diálogo nativo de carpetas).

### Arquitectura en una mirada

```
main.rs  → crea NuIdeWindow, Ide::shared(), editor::attach(), run()
editor.rs (Ide)  → dueño del estado y del "wiring" con Slint
  ├── explorer.rs   árbol plano con profundidad (raíz = proyecto)
  ├── highlight.rs  tokenizador de SpASM por línea
  ├── config.rs     persistencia mínima (carpeta del proyecto)
  ├── spasm.rs      descubrimiento del toolchain (spasm/spasmc/cc/opencode)
  ├── pty.rs        PTY real (ConPTY) + hilos de lectura/escritura
  ├── vt.rs         emulador VT (vt100) → tramos con color, scrollback, selección
  └── terminal.rs   consola lineal heredada (sólo la usan pruebas de integración)
ui/*.slint          la ventana, paneles y tema
```

- Todos los procesos corren en hilos y avisan por canales (`std::sync::mpsc`).
- Un `Timer` de 60 ms drena los canales, aplica los eventos y llama a `sync()`,
  que vuelca el estado a la ventana. El timer usa `Weak` para no crear un ciclo.
- Las terminales comparten emulador y gestos mediante un enum `Target { Local,
  Agent }`; sólo cambian el proceso que corre y el panel donde se dibujan.

---

## 2. Qué funciona hoy

- **Explorador**: árbol real, abrir/cerrar carpetas, abrir archivos en pestañas,
  icono de engranaje para configs, topes (500/carpeta, 4000 nodos).
- **Editor**: edición, guardado, deshacer/rehacer, selección, copiar/cortar/pegar,
  auto-indentación y **resaltado por lenguaje** (se detecta por extensión: SpASM,
  Slint, JSON, TOML, YAML, Markdown, shell, Rust; el lenguaje va en la barra de
  estado). **Buscar y reemplazar** (`Ctrl+F` / `Ctrl+H`) con contador, navegación,
  mayúsculas, **palabra completa** y edición del campo con cursor. Resaltados de
  contexto: palabra bajo el cursor y par de llaves. **Diagnóstico real**: corre el
  parser de SpASM y marca el error en el margen y la barra de estado. `F5` ejecutar
  / `Ctrl+B` compilar / `F6` analizar / `F8` navegar diagnósticos. **Build/Run**
  captura la salida del toolchain en una pestaña `build` y marca los errores con
  su línea. **Visores**: imágenes (zoom/paneo) y **vista previa de Markdown**
  (`Ctrl+Shift+M`).
- **Terminal local**: shell persistente en ConPTY, varias sesiones, resize,
  scrollback de 2000 líneas, selección y copia con el mouse.
- **Panel del agente**: `opencode` corriendo en un PTY (su TUI real), con los
  mismos gestos que la terminal local.
- **Proyecto**: se elige con el diálogo nativo (clic en el nombre en la barra
  superior) y se recuerda entre arranques.
- **Layout modular**: cada panel (Explorador, OpenCode, Terminal) tiene un
  **dock** (izquierda/derecha/abajo/oculto) y los que comparten lado se **apilan**
  (hasta 3). Se controla desde los botones de layout de la barra o el menú Ver; el
  editor ocupa el resto y el dock se recuerda.
- **Barra de estado**: lenguaje, codificación y posición del cursor reales. La
  rama está fija en `main` (ver §4).

---

## 3. Cambios de esta iteración (2026-10-04)

### 3.1 Terminal: scrollback + selección/copia (`c53fc50`)

- `vt.rs`: el emulador guarda **2000 líneas** de historial. Nuevas operaciones
  `scroll_by`, `scroll_to_end`, `scrollback_total` y `text_between`; el
  `snapshot` acepta una región de selección y pinta su fondo.
- `editor.rs`: rueda del mouse y `Shift+PageUp`/`PageDown` recorren el historial;
  arrastrar selecciona y al soltar **copia al portapapeles**; `Ctrl+Shift+C`
  recopia y `Ctrl+Shift+V` pega. Cualquier tecla normal vuelve al final.
- UI: barra fina de scroll a la derecha y `TouchArea` de selección en
  `TerminalScreen`.

### 3.2 Panel del agente con PTY real (`ba8e52c`)

- El panel derecho deja `opencode run` (un proceso por prompt) y corre `opencode`
  en un PTY, con el mismo emulador `vt`.
- El agente pasa a ser un `LocalSession` con canal propio
  (`agent_tx`/`agent_rx`); los gestos se comparten con la terminal local vía
  `Target`.
- `IdeOpenCode` usa `TerminalScreen`. Se eliminaron `TerminalBody` e
  `IdeTermLine`; `spasm.rs` perdió `agent_command`.

### 3.3 Elegir proyecto (`d775c3d`)

- Se agregó `rfd` y el nombre del proyecto en la barra superior quedó clickeable.
- `src/config.rs`: persistencia `project=<ruta>` en `%APPDATA%\NuIde\config`
  (o `$XDG_CONFIG_HOME`/`~/.config` fuera de Windows). Se lee al arrancar; si la
  carpeta ya no existe, se ignora.
- Al elegir otra raíz: se reconstruye el explorador, se **relanza el agente** en
  la carpeta nueva y se guarda la elección.

### 3.4 Editor: buscar/reemplazar y resaltados

- Se agregó la **barra de búsqueda** (`Ctrl+F`, `Ctrl+H`): contador
  `actual/total`, siguiente/anterior, distinguir mayúsculas, **palabra completa**,
  reemplazar la actual y reemplazar todo (con deshacer). Los campos se editan con
  cursor (flechas, Home/End, Backspace/Delete). La barra usa el mismo teclado
  crudo que el editor: `Enter`/`Shift+Enter`, `Tab` y `Esc`.
- **Atajos**: `F5` ejecuta el archivo activo y `Ctrl+B` lo compila (además de los
  menús de la barra superior).
- **Resaltados de contexto**: palabra bajo el cursor (todas sus apariciones) y
  par de llaves `()[]{}` correspondiente.
- El render se reordenó: un `build_row` aplica por carácter las decoraciones
  (selección, palabra, llaves, coincidencias) y el cursor. De paso se corrigió el
  mapeo de **tabs**: el resaltado expande a 4 espacios y ahora el cursor y las
  selecciones se mapean a esa coordenada (antes quedaban corridos en líneas con
  tabs).

### 3.5 Layout modular y menú Ver

- La ventana calcula su geometría según el **dock** y la visibilidad de cada
  región (`show-left`, `show-right`, `show-bottom`): reparte la altura entre los
  paneles apilados, el editor ocupa el resto y los divisores de las regiones
  vacías dejan de responder.
- El ítem **Ver** de la barra superior abre un desplegable (`ui/ide_viewmenu.slint`)
  con **Explorador**, **OpenCode CLI** y **Terminal**; cada fila muestra el dock
  actual y al hacer clic lo cicla (izquierda → derecha → abajo → oculto). El menú
  se ancla bajo el ítem `Ver` (la barra publica su x) y se cierra al hacer clic
  afuera.
- El estado de visibilidad vive en `Ide` y lo vuelca `sync`.
- La barra superior suma **botones de layout** (marco con la banda del área,
  en menta cuando está visible) para las tres regiones; el menú **Ver** se abre
  desde el ítem de la barra de menús (no hay botón duplicado).
- El menú Ver cicla el **dock** de cada panel (izquierda → derecha → abajo →
  oculto) y muestra dónde está. La geometría de la ventana calcula cuántos paneles
  hay por región y reparte la altura entre los apilados. Dock y visibilidad de
  región se persisten en la config.

### 3.6 Diagnóstico real con el parser de SpASM

- `src/lint.rs` corre el frontend de SpASM (`spasm-asm-gen.py`: tokenizer, parser
  y chequeo de tipos) con Python, en un hilo, y devuelve el error con su línea.
  No es un language server: parsea el archivo y reporta el **primer** error.
- Se dispara al **abrir**, al **guardar** y con **`F6`**. El resultado se drena
  por un canal en el timer de 60 ms.
- En el editor, la línea con error se marca con **barra y número rojos**
  (`IdeCodeLine.error`) y el mensaje va a la **barra de estado**.
- `spasm.rs` descubre el frontend junto a `spasmc.py` (`src/common/…`), en
  `F:\SpAMS-Oficial` o por `NUIDE_SPASM_FRONTEND`. Test de punta a punta real
  (se saltea si falta Python o el frontend).

### 3.7 Terminales: grilla exacta, ancho de celda medido y fuente

- La TUI de `opencode` se desalineaba porque el ancho de celda asumido
  (`7.2 px`) no coincidía con el avance real de la fuente.
- **Causa raíz**: Slint **no** soporta listas de fuentes separadas por comas
  (`font-family` es un único nombre de familia; el resto cae al fallback
  genérico). El tema declaraba `"JetBrains Mono, Cascadia Mono, Consolas,
  monospace"`, que no matcheaba ninguna → se usaba una **proporcional** (celda de
  ~10 px) en el editor y las terminales.
- **Arreglo**: `editor::attach` elige **una** familia instalada (`pick_font`:
  JetBrains Mono / Cascadia Mono / Consolas; y Montserrat / Century Gothic /
  Segoe UI) y la setea en el global `IdeTheme`. La ventana **mide** una celda
  (32 `M` a 12 px) y Rust recalcula las **columnas del PTY** con esa medida.
- `TerminalScreen` dibuja cada tramo en su **columna** (`run.col * cell-width`,
  ancho `run.cols * cell-width`, con clip). Así los glifos de caja/bloque
  (`▀ █ ┃ ╹`) caen en la grilla y no se superponen.

### 3.8 Editor: resaltado por lenguaje

- `highlight.rs` define `Language` (detectado por extensión) y `tokenize_line(line,
  lang)`. Tokenizadores: **SpASM**, **Slint**, **JSON**, **TOML**, **YAML**,
  **Markdown**, **shell**, **Rust**, **Python**, **JavaScript**, **TypeScript**,
  **C**, **C++**, **C#**, **Java**, **Go**, **Kotlin**, **Swift**, **Lua**,
  **Ruby**, **PHP**, **HTML**, **XML**, **CSS** y **SQL**; el resto va texto plano.
- Cada `OpenTab` guarda su `Language`; `build_row` resalta con el del archivo y la
  **barra de estado** muestra el lenguaje real (antes decía siempre "SpASM").
- Los tokenizadores genéricos se parametrizan con un `Spec` (comentarios, comillas,
  keywords, `@macro`, `$var`, `#color`, `[sección]`, clave antes de `:`/`=`).

### 3.9 Visores: imágenes y vista previa de Markdown

- Las imágenes (`.png`, `.jpg`, `.webp`, `.gif`, `.bmp`, `.ico`) se abren como
  pestañas `DocKind::Image`: se cargan con `slint::Image::load_from_path` y se
  muestran con un `Image` (`image-fit: contain`) dentro de un `Flickable`, con
  **zoom** por pestaña (`image_zoom`, botones `−`/`+`/`Ajustar`, de 0.1× a 8×) y
  paneo arrastrando.
- Los `.md` tienen vista previa. Un botón **Vista / Código** (o `Ctrl+Shift+M`)
  alterna. `markdown_preview` convierte a `slint::StyledText` (negrita, itálica,
  código, links, listas); como `StyledText` no soporta headings ni cercos, se
  convierten/omiten y, si igual falla el parseo, cae a texto plano.

### 3.10 Build/Run desde la IDE con errores navegables

- `src/build.rs` corre el toolchain (`spasmc`/`spasm`) con `Command`, **captura**
  stdout+stderr y **parsea** los diagnósticos: formato SpASM (`linea N: …`) y
  gcc/clang (`archivo:N:col: error: …`).
- La salida cruda va a una sesión de terminal dedicada **`build`** (un
  `LocalSession` sin PTY, alimentado por `Screen::process`), a la que la IDE salta
  al compilar.
- Los diagnósticos se muestran con la línea en rojo y `F8`/`Shift+F8` navegan
  entre ellos.
- Los menús Ejecutar/Compilar ya no escriben en la shell: ejecutan y capturan.

### 3.11 Esquema (outline) e ir a definición

- La misma corrida de `spasm-asm-gen.py` devuelve, además de los diagnósticos, el
  **esquema** del AST: funciones (`("func", nombre, …, línea, ret)`) y variables
  `let`/`recurso` (walk recursivo).
- `Ide.outline` lo vuelca a `IdeOutlineItem`; el editor tiene un botón **Esquema**
  con un desplegable (funciones en menta, variables normales) cuya fila salta a
  la línea (`goto-line`).
- **`F12`** va a la definición: toma el identificador bajo el cursor (`word_at`) y
  busca el símbolo con ese nombre en el esquema.

### 3.12 Terminal inferior dividida y agente sin reinicio

- **Split**: el panel inferior muestra dos `TerminalScreen` lado a lado
  (`IdeTerminal.split-view` + `lines-2`); la segunda vista usa otra sesión. El
  botón «Dividir» alterna; `focus_pane` intercambia activo/dividido (clic en una
  vista le da el foco). En el split, el PTY de cada panel usa la **mitad** del
  ancho (`resize_local`).
- **Cambiar de proyecto no reinicia `opencode`**: `set_project` llama a
  `retarget_agent`, que le manda `/cd <carpeta>` al PTY del agente (comando del
  TUI de opencode); si no está vivo, lo arranca en la raíz nueva.

### 3.13 Página de bienvenida del editor

- Sin pestañas abiertas, el área del editor muestra `ui/ide_welcome.slint`
  (`IdeWelcome`) en vez del código: título, accesos **Abrir carpeta…** /
  **Abrir archivo…** / **Nuevo archivo…**, la lista de **Recientes** y los
  atajos.
- `welcome` lo setea `sync` (`tabs.is_empty()`); `pick_file` usa `rfd::pick_file`
  y `new_file` usa `rfd::save_file` (crea la plantilla SpASM si no existe).
- **Recientes**: `recent_projects` y `recent_files` (tope 8, sin duplicados) se
  guardan en la config separados por `|`; la bienvenida manda el índice y
  `open_recent` abre el proyecto (`set_project`) o el archivo.
- Se quitó el bloque de código demo del `.slint` (ya no hace falta).

### 3.14 Control de código (Git)

- Módulo `src/git.rs`: ejecuta la CLI de Git (sin dependencias). `status` parsea
  `git status --porcelain=v1 -b` (rama + cambios); `perform` hace `init`,
  `add -A`, `add`/`reset` por archivo, `commit -m` y `push`. `spawn` lo corre en
  un hilo y avisa por el canal `git_tx`/`git_rx` (igual que `lint`/`build`).
- Panel `ui/ide_git.slint` (`IdeGit`): rama, secciones **CAMBIOS** y
  **PREPARADOS**, botón **+**/**−** por fila (stage/unstage), clic para abrir,
  **Preparar todo**, **Commit** (activo con mensaje) y **Push**; **⟳** refresca.
  Si no es repo, **Inicializar repositorio** (`git init`).
- El panel es el 4.º dock (índice 3): `docks: [Dock; 4]`, `panel.3.dock` (por
  defecto izquierda, apilado con el explorador). El menú Ver lista
  «Control de código».
- `git_busy` bloquea operaciones simultáneas; el estado se relee al abrir un
  proyecto, al guardar y al terminar cada operación. La rama se muestra en la
  barra de estado (`git-branch`).

### 3.15 Paleta de comandos y buscador de archivos

- `Ctrl+Shift+P` abre la **paleta de comandos** y `Ctrl+P` el **buscador de
  archivos**; `Esc` cierra, `↑`/`↓` mueven y `Enter` ejecuta (o abre).
- Overlay `ui/ide_palette.slint` (`IdePalette`): fondo oscurecido, campo de
  filtrado y lista con auto-scroll de la fila seleccionada.
- El estado y el teclado viven en Rust (`Palette`, `PaletteKind`, `Command`):
  `palette_intercept` se llama al inicio de `editor_key` y `pty_key`, así el
  atajo es global (el foco puede estar en el editor, la terminal o el agente).
  Mientras está abierta, las teclas se desvían a `palette_key`.
- Comandos: ejecutar/compilar/analizar, siguiente error, buscar/reemplazar, ir a
  definición, vista previa, guardar, abrir carpeta/archivo/nuevo, buscar
  archivos, nueva/dividir terminal, `Ver:` cada panel y refrescar/preparar/push
  de Git. `filter_commands` matchea por etiqueta o atajo.
- `collect_files` junta los archivos del proyecto (recursivo, tope 6000, sin
  `target`/`node_modules`/`.git`/… ni carpetas ocultas); el buscador filtra por
  ruta y muestra hasta 50.

### 3.16 Menús Archivo/Editar y flujo de archivos

- El menú superior dejó de ser sólo "Ver": `menu()` alterna el desplegable
  abierto en `menu_open` ("Archivo"/"Editar"/"Ver") y `sync` manda a la UI las
  filas (`IdeMenuEntry`) y el ancla. `ui/ide_menu.slint` (`IdeMenu`) dibuja
  cualquier menú; las filas las arma `file_menu_items`/`edit_menu_items`.
- **Archivo:** nuevo, abrir archivo/carpeta, guardar, **guardar como** (escribe
  y reapunta la pestaña: ruta, idioma, migas y diagnósticos), cerrar pestaña,
  cerrar todas y salir. **Editar:** deshacer/rehacer, cortar/copiar/pegar,
  seleccionar todo, buscar/reemplazar e **ir a línea**.
- Atajos nuevos en `editor_key`: `Ctrl+N`, `Ctrl+O`, `Ctrl+Shift+S`, `Ctrl+W` y
  `Ctrl+G`. Todo también está en la paleta; "Ir a línea" es la paleta en modo
  numérico (`PaletteKind::GoToLine`, sólo dígitos).
- **Cierre seguro:** cerrar una pestaña/todas, salir o cerrar la ventana con
  cambios sin guardar abre `ui/ide_confirm.slint` (`IdeConfirm`) con
  **Guardar / Descartar / Cancelar** (`Confirm`, `confirm_save/discard/cancel`,
  `save_tab`).

### 3.17 Explorador: operaciones de archivos

- **Menú contextual** (clic derecho en una fila, `PointerEventButton.right`):
  Nuevo archivo/carpeta, Renombrar, Eliminar (deshabilitados en la raíz),
  Copiar ruta, Mostrar en el explorador y Refrescar. Las filas las arma
  `explorer_context`; el menú se dibuja a nivel de ventana usando las
  coordenadas locales que publica `IdeExplorer` (`context-x`/`context-y`), así
  no lo recorta el panel.
- **Diálogo de entrada** `ui/ide_prompt.slint` (`IdePrompt`) para nuevo/renombrar;
  el texto y el cursor los maneja Rust (`Prompt`, `prompt_key`, validación con
  `valid_name`). Crear abre el archivo nuevo; renombrar reapunta las pestañas
  (`retarget_open_tabs`).
- **Eliminar** pide confirmación (`IdeAsk`, rojo) y cierra las pestañas que
  quedaron dentro de lo borrado. Hoy borra de forma permanente (sin papelera).
- **Encabezado**: **⟳** refresca y **◍** muestra/oculta generados
  (`Explorer::set_show_generated`, filtro en `walk`); el estado se guarda en
  `explorer.generated`.
- El explorador se reconstruye al crear/renombrar/borrar y al guardar como.

### 3.18 Editor avanzado (búsqueda, edición y ajustes)

- **Búsqueda** ampliada en `Find`: botón `.*` compila la consulta con
  `regex-lite` (`build_regex`, `find_in_line_regex`, case por bandera); botón
  `Sel.` acota a la selección (captura el rango al activar y filtra por
  línea/columna); `↑`/`↓` recorren el **historial** (`remember_query`,
  `find_history`) en el campo de búsqueda.
- **Auto-cierre de pares**: `insert_char` inserta `()`, `[]`, `{}`, `""`, `''`
  y deja el cursor en el medio; escribir el cierre presente sólo avanza
  (`skip_over`). Las comillas dentro de una palabra no se autocierran.
- **Autoindentación**: `newline` abre un nivel tras un abridor (`(`/`[`/`{`) y
  alinea el cierre; usa la unidad de indentación configurada.
- **Tab configurable**: `tab_size` + `insert_spaces` (config
  `editor.tab-size` / `editor.insert-spaces`), comandos de paleta «Editor:
  tamaño de tab (2/4/8)» y «Editor: insertar espacios». `Tab` inserta la unidad.
- **Persistencia de cursor** por pestaña (`tab.{i}.cursor` en la config); se
  recupera al reabrir. Cambiar de pestaña ya conservaba el cursor.
- **Dependencia**: se sumó `regex-lite` (variante liviana de `regex`, sin tablas
  Unicode) — primera dependencia nueva justificada por la búsqueda.

### 3.19 Terminal: mouse reporting y sesiones

- `vt::Screen` expone `mouse_protocol_mode`/`mouse_protocol_encoding` (paso
  directo a `vt100`). `Ide` guarda, por sesión, el último botón apretado
  (`mouse_button`).
- `forward_mouse` decide si reportar según el modo (`Press`, `PressRelease`,
  `ButtonMotion`, `AnyMotion`) y codifica con `encode_mouse` en **SGR**
  (`\x1b[<b;col;rowM/m`), **UTF-8** o **X10** (`\x1b[M` + 3 bytes).
- En la UI, `TerminalScreen` tiene `mouse-tracking`: cuando está activo, los
  eventos de la `TouchArea` van a `mouse-report(kind, button, y, x)` en vez de a
  la selección/scrollback. `IdeTerminal` e `IdeOpenCode` lo reenvían; la ventana
  llama `forward_mouse(Local|Agent, …)`.
- **Sesiones**: comandos de paleta `Terminal: renombrar sesión…` (reusa
  `IdePrompt` con `PromptKind::SessionName`) y `Terminal: reiniciar sesión`
  (`restart_session`, conserva la posición y arranca en la raíz del proyecto).

### 3.20 Buscar en todo el proyecto

- Módulo `src/search.rs`: `spawn` corre la búsqueda en un hilo y manda
  `SearchResult { gen, hits, truncated }`; `gen` permite descartar respuestas
  viejas. Reutiliza `collect_files` y los buscadores de línea del editor.
- Panel `ui/ide_search.slint` (`IdeSearch`): campo de búsqueda, campo de
  reemplazo opcional, botones `Aa` (mayúsculas), `.*` (regex), `⇄` (reemplazo) y
  **Reemplazar todo**; lista de resultados `ruta:línea  texto` con auto-scroll.
- Se abre con **`Ctrl+Shift+F`** (`search_intercept`, global) o el comando de
  paleta. El teclado lo maneja Rust (`search_key`); `Enter`/clic abren el archivo
  en la línea (`search_accept`).
- **Reemplazar en el proyecto** pide confirmación (`Ask::ReplaceInProject`,
  generalizamos `ask` a un enum), aplica `replace_in_line` o la regex, recarga
  las pestañas abiertas afectadas (`reload_open_tab`) y refresca Git.

### 3.21 Git avanzado

- `git.rs`: `Status` ahora trae `ahead`/`behind` (se parsean del `## ...`
  `[ahead N, behind M]`). Operaciones nuevas: `UnstageAll`, `Pull`,
  `Amend(String)`, `Discard { path, untracked }`, `SwitchBranch`, `CreateBranch`.
  Helpers `diff(root, path)` (unificado, `HEAD`→working tree; sintetiza el
  archivo nuevo) y `branches(root) -> [(nombre, actual)]`.
- **Panel**: encabezado con `↑ahead ↓behind` y la rama **clickeable** → menú
  (`IdeMenu`) con las ramas locales (la actual con `●`) y «Nueva rama…»
  (`PromptKind::NewBranch`). Pie en dos filas: **Preparar todo** / **Quitar
  todo** y **Bajar** / **Ramas** / **Commit**. Clic en una fila abre el **diff**.
- **Visor de diff** `ui/ide_diff.slint` (`IdeDiff`): líneas tipadas por
  `parse_diff_lines` (contexto/agregada/borrada/hunk/metadatos), y botones
  **Abrir archivo**, **Preparar/Quitar** y **Descartar cambios**
  (`Ask::Discard`, con confirmación).
- Comandos de paleta: `Git: bajar`, `Git: quitar todo del índice`,
  `Git: enmendar…`, `Git: ver diff del archivo`.

### 3.22 Flujo SpASM (config de ejecución, plantillas y ayuda)

- **Config de ejecución**: `run_entry` (archivo de F5; por defecto el activo),
  `run_args` (se parten con `shell_split` respetando comillas) y
  `run_build_on_save`. Se guardan en `run.entry` / `run.args` /
  `run.build-on-save`; `run()` los usa. Comandos de paleta para elegir la
  entrada (`rfd`), volver al activo, editar los argumentos (`PromptKind::RunArgs`)
  y alternar compilar al guardar (que dispara `build` desde `save_active`).
- **Plantillas** (`src/templates.rs`): `desarrollo` (intérprete, `F5`), `release`
  (compilado, `Ctrl+B`) y `vacio`. `new_file_with(body)` crea el archivo con el
  cuerpo; comandos de paleta por plantilla y `new_project` (crea
  `src/main.spasm` de desarrollo, fija el proyecto y lo abre).
- **Flujo intérprete/compilador**: `F5` usa el intérprete (dialecto de
  intérprete); `Ctrl+B` compila (release). Si el archivo de entrada usa
  `programa`, `run()` lo detecta (`uses_compiled_dialect`) y avisa en la sesión
  `build` en vez de dejar que el intérprete falle.
- **Ayuda de sintaxis**: `PaletteKind::Help` lista `highlight::spasm_words()`
  filtrado por la palabra bajo el cursor y, al elegir, **inserta** el término
  (`PaletteAction::InsertWord`). Atajo `F1`.
- **Depurador: bloqueado por el toolchain.** `spasm.exe` no expone paso a paso,
  breakpoints ni inspección de registros (`--help` no los lista), así que un
  depurador interactivo requiere extender el intérprete; queda documentado.

### 3.23 Preferencias y atajos

- Panel `ui/ide_prefs.slint` (`IdePrefs`), abierto con `Ctrl+,`/comando:
  Apariencia (fuente del código y de la interfaz, tamaño del código con `−`/`+`,
  acento), Editor (tab, insertar espacios), Ejecutar (compilar al guardar) y
  Atajos.
- **En caliente**: `apply_theme` toca los globales `IdeTheme` (sans, mono,
  `editor-font-size`, `mint`). El tamaño del código se aplica al editor vía
  `IdeTheme.editor-font-size` (también cambia el alto de fila).
- **Atajos configurables**: `KEY_ACTIONS` (paleta, archivos, búsqueda, ayuda) da
  los `chord_of(text, ctrl, shift, alt)` → `keymap`; `capture_key` captura la
  combinación nueva (Esc cancela) y la guarda en `keys.*`. `keymap_action` +
  `run_key_action` los despachan al inicio de `editor_key`/`pty_key`.
- Persistencia: `font.sans`, `font.mono`, `font.editor-size`, `theme.accent`,
  `keys.*`.
- **Pendiente:** i18n y permitir reasignar *todos* los atajos (hoy los globales).

### 3.24 Icono, instalador y extras

- **Compilación nativa**: `spasm.rs` detecta un compilador de C (`CCompiler::Direct`
  para gcc/clang/cc/tcc o `Msvc` con `vcvars64.bat` vía `vswhere`). `build()` hace
  dos pasos: transpila (`transpile_command`) y compila el `.c` a binario
  (`native_command`, con un `.bat` temporal para MSVC). `build::spawn` ahora corre
  una **cadena de pasos**. `Ctrl+B` compila; `Ctrl+Shift+B` compila y ejecuta.
- **Sin consola en release**: `main.rs` usa
  `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` para que el
  `.exe` no abra una terminal vacía; `attach_console()` engancha la consola del
  padre sólo para `--version`/`--help`.

- **Menú Ayuda** + **Acerca de** (`ui/ide_about.slint`): versión
  (`CARGO_PKG_VERSION`), créditos y enlace; `menu_command` resuelve `help.*` y
  `open_url` abre el navegador.
- **Medidor de sistema** (`src/sysinfo.rs`): FFI de `kernel32`
  (`GetSystemTimes`, `GlobalMemoryStatusEx`) en Windows y `/proc` en el resto;
  el pie muestra `CPU n% RAM n%` discreto (muestreo cada ~1,5 s en `drain`).
- **Icono y versión del `.exe`**: `res/nuide.ico` + `res/nuide.rc`; `build.rs`
  compila el `.rc` con `rc.exe` (Windows Kits, o `RC`) y lo enlaza; sin `rc.exe`
  avisa y sigue.
- **Instalador Inno Setup** (`packaging/NuIde.iss`) + `packaging/postinstall.ps1`:
  instala la app, accesos directos, asociación `.spasm` opcional, y ofrece
  **Node.js** (winget) y las **CLI de IA** (npm).
- **CI** (`.gitlab-ci.yml`): `fmt`/`clippy -D warnings`/`test` en Linux y `.exe`
  release como artefacto en Windows. Perfil `release` optimizado.

### 3.25 Performance: refresco incremental y renderer

Medición base (app abierta, sin tocar nada, con el panel de OpenCode corriendo):
**77 % de un núcleo y 113 MB**. Después de estos cambios, mediana sobre 18 s:
**10,4 % y 40 MB** (sin la TUI del agente: 9,4 % y 34,6 MB).

- **Renderer por defecto = software** (`src/main.rs`, `pick_renderer`): Slint lee
  `SLINT_BACKEND` y acá el orden por defecto elegía FemtoVG (OpenGL), que repinta
  la escena **completa** ante cualquier cambio. Mismo binario, sólo cambiando la
  variable: `winit-femtovg` → 77 % / 113 MB; `winit-software` → 10 % / 30 MB (el
  renderer software hace *partial rendering* de la región dañada y no depende del
  driver OpenGL). Se puede seguir probando la vía GPU con
  `$env:SLINT_BACKEND = "winit-femtovg"`.
- **Tramos por huella de fila** (`src/vt.rs`): `Screen` guarda una huella (`u64`)
  por fila y `changed_rows()` devuelve **sólo las filas que cambiaron**. El
  recorrido de celdas para la huella no asigna memoria (`Cell::contents()` es
  `&str`; antes se hacía un `String` por celda en cada refresco). `snapshot()`
  sigue disponible (lo usan las pruebas) y `resize()` invalida las huellas.
- **Modelo de filas mutable en su sitio** (`src/editor.rs`, `TermRows`):
  `VecModel` no tiene "cambiar fila"; el `remove` + `insert` mandaba
  `row_removed`/`row_added` y Slint **destruía y recreaba** el elemento del `for`
  con sus `Text` (el shaping completo). `TermRows` implementa `Model` sobre un
  `RefCell<Vec<TermLine>>` y avisa con `row_changed` → el elemento se reutiliza.
  Comprobación empírica: con el mismo cómputo Rust y **sin** mutar el modelo, el
  CPU en reposo era 3 %; con la mutación por `remove`+`insert`, 77 %.
- **Refresco segmentado** (`sync_terms`): la salida del PTY ya no llama al
  `sync()` global (que reconstruye ~20 modelos de la ventana: explorador,
  búsqueda, diff, paleta…). `drain()` distingue salida nueva (`Output`) de un
  cambio estructural (`Exit`, linter, build, git, búsqueda) y refresca sólo los
  paneles de terminal.
- **El medidor de CPU/RAM no fuerza redraw**: escribe directo
  `cpu-percent`/`ram-percent` en vez de poner `got = true` (antes provocaba un
  vuelcado completo cada ~1,5 s *aunque no pasara nada*).
- **Paneles ocultos no se refrescan** (`dock_visible`): si el dock de la terminal
  o del agente está cerrado o su región está apagada, se saltea el trabajo; al
  volver, el `sync` completo los pone al día.
- **Pruebas**: `vt::tests::el_refresco_incremental_solo_trae_las_filas_cambiadas`
  fija que el refresco incremental devuelva lo mismo que el snapshot completo y
  que no reconstruya toda la pantalla.

**Queda pendiente** (diagnóstico hecho, no implementado): `push_undo` clona el
documento entero por carácter; `decorations()` es O(documento) por fila
actualizada; la selección y `Enter` ponen `editor_full` (re-render total); el
editor no virtualiza (20k componentes reales); `git::diff`/`branches`/`status`
corren subprocesses síncronos en el hilo UI; la búsqueda en proyecto no tiene
*debounce* (un escaneo completo por tecla).

---

### 3.26 Reorganizar la ventana: arrastrar paneles y opacidad

- **Drag & drop de docks** (`ui/ide_dock.slint`): un global `DockDrag`
  (`panel`/`px`/`py`/`label` + `callback released()`) y un `DockHandle` —el asa
  con tres rayitas— plantado en la cabecera de los cuatro paneles, declarado
  **antes** que los botones para no comérselos. La ventana (`nuide_ui.slint`)
  traduce el puntero a una **banda** (`zone-left` / `zone-right` / `zone-bottom`,
  y el centro = ocultar), pinta la guía y una etiqueta que sigue al cursor.
  Rust conecta el global con `window.global::<DockDrag>().on_released(...)`
  (conectar un callback de un global desde el cuerpo de un componente **no** es
  sintaxis válida en Slint) y aplica `Ide::dock_drop(panel, zona)`, que además
  **enciende la región de destino** si estaba apagada. Persiste por el mismo
  camino que el menú Ver: `save_state` → `panel.<i>.dock`. Prueba:
  `soltar_un_panel_arrastrado_lo_cambia_de_dock`.
- **Opacidad de los paneles**: `IdeTheme.panel-alpha` + `opacity:` en la superficie
  de los cuatro paneles; **Preferencias → Opacidad de los paneles** cicla
  `PANEL_ALPHAS` (100/92/85/75/65 %) y se guarda en `theme.panel-alpha`.
  **Límite honesto**: no hay transparencia al escritorio — el backend winit de
  Slint arma la ventana principal con `with_transparent(false)`
  (`winitwindowadapter.rs`), así que sólo se compone contra el fondo de la
  ventana.
- **Copy/paste en la columna derecha** (venía de §3.24 pero la solución quedó
  acá): con *mouse reporting* la TUI se queda con el mouse y no se podía
  seleccionar. Ahora **Shift+arrastrar** libera el mouse y selecciona (se copia al
  soltar), y el **botón del medio** pega (`paste-requested` → `paste_into`).



### 3.27 Despliegue del sitio (FTP/FTPS/SFTP) sin crates de red

**Decisión**: nada de `suppaftp`/`ssh2`/`russh` (el usuario lo pidió explícito:
"las librerías de Rust son un infierno"). Windows ya trae `curl.exe`
(`System32`, habla `ftp`/`ftps`/`sftp`) y OpenSSH (`ssh`, `scp`, `sftp`). Las
transferencias son procesos hijos, como el resto del toolchain.

- `src/site.rs`: `Site` (transporte, host, puerto, usuario, contraseña, carpeta
  remota, subcarpeta local, extensiones, script, `cacert`) y `Transport`
  (FTP/FTPS/SFTP con puerto y esquema propios). **Se guarda en el config de NuIde
  indexado por proyecto** (`site.<ruta>.host`), así dos proyectos no se pisan y
  nunca se escribe dentro del repositorio (donde una credencial se comitearía).
  `url_for` / `curl_args` / `scp_args` / `ssh_args` arman los comandos; FTPS usa
  la validación de certificado que hace curl por defecto y acepta una CA propia
  con `--cacert`.
- `src/deploy.rs`: el trabajo pesado en un hilo que habla por canal
  (`DeployMsg::{Log,Sent,Aborted,Finished}`) y se drena en la sesión `build` de
  la terminal. `collect()` filtra por extensión evitando `.git`, `target`,
  `node_modules`; `plan()` resuelve las rutas que pidió el script.
  `spawn_test()` prueba el sitio con `curl --list-only` sin subir nada.
- **La lógica de *qué* subir puede ser un script SpASM del proyecto**
  (`deploy.spasm`, ejecutado con el intérprete que ya detecta `spasm.rs`, igual
  que F5). El protocolo es por líneas — el mismo estilo que el host de SpASM usa
  con `popen` (`spasm-gui.c:1984`):
  `UPLOAD <local> [<remota>]`, `SKIP <local> [motivo]`, `LOG <texto>`,
  `ERROR <texto>`, con `>` opcional. NuIde ejecuta la transferencia, así el script
  no necesita un builtin para lanzar procesos. Sin script: se sube la subcarpeta
  filtrando por extensión.
- UI: sección **Despliegue** en Preferencias (clic en un campo abre el diálogo de
  entrada existente; `PromptKind::SiteField`, exenta de `valid_name` porque un
  host o una contraseña no son nombres de archivo) con Transporte/Host/Usuario/
  Contraseña/Carpeta remota/Subcarpeta local/Extensiones/Script y los botones
  **Probar conexión**, **Subir cambios** y **Terminal SSH** (esta última abre una
  sesión `ssh user@host` en el PTY real).
- 13 pruebas nuevas entre `site::tests` y `deploy::tests` (125 en total). Una de
  ellas detectó un bug real: `scp_args` reaplicaba `remote_root` sobre una ruta
  ya resuelta y duplicaba el prefijo.



### 3.28 Módulos opt-in: servidor web con CORS y visor de SQLite

Los dos se prenden/apagan desde **Preferencias → Módulos** y no agregan ni una
dependencia al build.

- **`src/webserver.rs`** — HTTP/1.1 con `std::net`, sin crates. Parseo y respuesta
  son funciones puras (`parse_request`, `respond`) y el hilo sólo acepta
  conexiones. Sirve la subcarpeta elegida del proyecto con MIME por extensión,
  `index.html` para directorios, y **CORS** configurable
  (`Access-Control-Allow-Origin/-Methods/-Headers`, preflight `OPTIONS` con
  `Max-Age`, y `Allow-Credentials` + `Vary: Origin` sólo cuando el origen no es
  `*`). El camino se resuelve **léxicamente** con `sanitize`: `..`, unidad (`C:`)
  o separador embebido devuelven 403 y no tocan el disco.
  - **Bug que encontró la prueba end-to-end**: `Server::stop()` hacía `join()` de
    un hilo clavado en `accept()` bloqueante → nunca volvía. Ahora el listener es
    no-bloqueante y sondea el flag de parada cada 20 ms.
  - `tests/webserver_e2e.rs` (`#[ignore]`, corre con `--ignored`): levanta el
    servidor en un puerto alto, pide un CSS, un directorio, un preflight, un
    traversal y un 404, y verifica status y cabeceras CORS de verdad.
- **`src/sqlite.rs`** — el visor **no** implementa el formato de SQLite (páginas
  B-tree, freeblocks: otro proyecto). Le pregunta a un binario que ya existe y le
  lee **TSV**: `sqlite3.exe` (PATH o `Android\Sdk\platform-tools`) o, si no,
  `python` con el módulo `sqlite3` de su biblioteca estándar (reusa el
  descubrimiento del toolchain SpASM, así respeta `NUIDE_PYTHON`). Mismo estilo de
  contrato por líneas que el despliegue. `probe()` valida el magic
  `SQLite format 3\0` antes de abrir nada, y `quote_ident()` escapa los nombres.
- **`ui/ide_sqlite.slint`** — panel modal en dos columnas (tablas | filas) con
  "Elegir base" y "Refrescar"; Rust manda los datos ya como texto, así que la UI
  no parsea ni modela filas.
- Cables en `editor.rs`: estado por módulo (`web`, `sqlite_*`), handlers de los
  callbacks, drenaje en `drain()` (las peticiones servidas se ven en la sesión
  `build`) y persistencia (`web.<proyecto>.*`, `module.sqlite`).
- 15 pruebas nuevas (10 de `webserver`, 5 de `sqlite`) más el e2e ignorado →
  **140** en la suite normal.

**Límites honestos**: el servidor escucha sólo en `127.0.0.1`, atiende una
petición por conexión (`Connection: close`) y no tiene rangos, caché ni
compresión; el visor muestra hasta 200 filas por tabla y no edita.


### 3.29 Panel del agente como terminal con opencode inyectado (`7f52ed4`)

- El panel derecho deja de ser un PTY que corre `opencode` directamente y pasa
  a ser una **shell normal** (como la terminal inferior) que lanza `opencode`
  automáticamente al arrancar.
- `start_agent()` lanza `pwsh`/`cmd` en el cwd del proyecto y, después de
  500ms (para que la shell muestre el prompt), inyecta el comando de opencode
  al PTY.
- Si el usuario sale de opencode (Ctrl+C, :q), queda en la shell y puede
  relanzarlo tipeando `opencode`, o usar la shell normalmente.
- `retarget_agent()` simplemente reinicia la sesión completa: la nueva shell
  arranca en el cwd del proyecto y lanza opencode automáticamente. Ya no
  dependemos del comando `/cd` de OpenCode (que es reciente y no todas las
  versiones lo tienen).
- **Ventaja**: al cambiar de proyecto, el agente siempre queda en el directorio
  correcto sin depender de comandos especiales. Si sale de opencode, tiene una
  terminal funcional.


### 3.30 Fix: mantener shell viva en panel del agente (`09b7a21`)

- **Problema**: cuando el usuario ejecutaba `/exit` en opencode, la shell
  también terminaba y mostraba `[proceso terminado: 0]` en lugar de volver al
  prompt de la shell.
- **Causa**: PowerShell, cuando se lanza sin `-NoExit`, puede salir después de
  ejecutar un comando si no está en modo interactivo. Al inyectar el comando
  `opencode`, la shell lo ejecuta y cuando opencode termina, la shell también
  termina.
- **Solución**: `start_agent()` ahora agrega `-NoExit` a los argumentos de
  PowerShell para que la shell se mantenga viva después de que opencode
  termine.
- **Resultado**: al salir de opencode, el usuario queda en la shell y puede
  tipear `opencode` para relanzarlo, ejecutar comandos de shell directamente,
  o usar la terminal normalmente.
- **Corrección en 3.31**: la causa real no era la interactividad; `-NoExit` no
  cambia el comportamiento y se retira.

### 3.31 Panel derecho como terminal pura

- El panel derecho **arranca como terminal real** (shell pwsh/cmd en la raíz
  del proyecto), igual que la inferior. `opencode` **ya no se inyecta al
  arrancar**: se lanza bajo demanda con la acción nueva **Lanzar opencode**
  (`agent.launch` en la paleta), que inyecta el comando de la CLI en la shell
  abierta.
- El comando se inyecta **entre comillas** (`& 'ruta'` en PowerShell,
  `"ruta"` en cmd) para que funcione aunque la ruta tenga espacios.
- Si el usuario sale de opencode, la terminal queda en la shell; desde la
  paleta puede relanzar opencode o reiniciar la terminal.
- **Auto-recuperación**: si la shell se cae, el panel **relanza la terminal
  sola** (la terminal inferior queda muerta a propósito).
- **Se retira el `-NoExit`** de `3.30`. Una prueba real en ConPTY mostró que
  ese flag no cambia el comportamiento: al salir de opencode con `/exit` la
  shell queda viva tanto con como sin `-NoExit` (con o sin el flag, el panel
  vuelve a ser una terminal); y si opencode se cierra con `Ctrl+C`, la shell
  muere igualmente (código 5) en ambos casos. La causa del cierre no es la
  "interactividad" de PowerShell sino el `Ctrl+C` de consola que opencode
  emite al salir, que arrastra a la shell del mismo grupo de consola.

### 3.32 Ejecutar compila (AOT); versión 0.2.0

- **Ejecutar (`F5`)** ya no usa el intérprete `spasm.exe`: ahora **compila** el
  `.spasm` a binario nativo (misma cadena que Compilar: `spasmc.py --target c`
  + compilador de C detectado) y corre el `.exe` con los argumentos
  configurados. Ambos dialectos (simple y `programa`) compilan; desaparece el
  aviso de "archivo con dialecto compilado".
- **Despliegue**: `deploy.spasm` también se **compila AOT** antes de correrlo
  (nuevo `Toolchain::compile_binary`). La IDE ya **no necesita el intérprete** en
  runtime; se elimina `interpreter_required` y `uses_compiled_dialect` (sin uso).
- Se comparte la cadena de pasos con `Ide::compile_binary_steps` (la misma que
  usa `Compilar`).
- Sin compilador de C, avisa y deja el `.c` (igual que Compilar).
- **Versión 0.2.0** en `Cargo.toml`, `res/nuide.rc` (FileVersion/ProductVersion)
  y `packaging/NuIde.iss` (AppVersion + VersionInfoVersion).

### 3.33 Proveedores de OpenCode desde la IDE (CRUD)

- Sección **Proveedores** en la configuración (`Ctrl+,`): lista los proveedores
  del CLI y permite **editar, crear y eliminar** con un formulario (ID, Nombre,
  Package, baseURL, apiKey y **Modelos** como JSON crudo).
- Lee la **unión** de `~/.config/opencode/opencode.json` y `opencode.jsonc`
  (gana la primera aparición del id); al guardar escribe todo en
  `opencode.json` (lo crea si falta) y **retira `providers`** del `.jsonc` para
  no dejar copias viejas.
- La escritura es **quirúrgica** (`src/jsonx.rs`, posiciones de bytes): sólo se
  reemplaza el valor de `providers`; `$schema`, `model` y el resto del archivo
  quedan byte a byte igual. Los campos del proveedor se parchean *in-place*, así
  que se conservan claves desconocidas (`env`, `modelID`, `limit`, `cost`…).
- **Validaciones al guardar:** el ID no puede estar vacío ni tener espacios,
  llaves o comas, y no puede repetirse; **Modelos** tiene que ser un objeto JSON
  válido. Si un proveedor resuelve sus credenciales por `env` y el campo apiKey
  queda vacío, **no** se le escribe `"apiKey": ""` encima (ni baseURL/package
  vacíos): sólo se pisa un campo si el usuario puso algo o el proveedor ya lo
  tenía.
- Eliminar pasa por el diálogo de confirmación (`Ask::DeleteProvider`).
- UI en `ui/ide_provider.slint`, montada encima de Configuración y debajo de los
  diálogos modales. `Esc` cierra; el área de Modelos es el único std-widget de la
  app (`TextEdit`, forzado a esquema oscuro).
- Tests nuevos: `src/jsonx.rs` (7) y `src/providers.rs` (4). Total **159** OK.

### 3.34 Versión 0.2.2 e instalador

- Bump a **0.2.2** en `Cargo.toml`, `Cargo.lock`, `res/nuide.rc`
  (FileVersion/ProductVersion), `packaging/NuIde.iss` (AppVersion +
  VersionInfoVersion) y `CHANGELOG.md`.
- `packaging/NuIde.iss`: la URL del publicador pasa al repositorio real
  (`github.com/nutoysaudio-code/NuIde`), que estaba en GitLab.
- Instalador generado con `packaging/make_installer.ps1` (Inno Setup 6):
  `releases/NuIde-0.2.2-setup.exe`.

### 3.35 Menú contextual en las terminales y elevación

- **Clic derecho** en las dos terminales (shell y panel de OpenCode) abre un menú
  con **Copiar** (la selección), **Pegar** y **Elevar a administrador** (se
  atenúa lo que no aplica). El `TerminalScreen` gana el callback
  `context-requested`; el menú se dibuja en la ventana, anclado a la terminal.
- **Elevación (Windows)**: nuevo `src/elevate.rs` (FFI a `ShellExecuteW` con el
  verbo `runas`, e `IsUserAnAdmin` para consultar). Dos comandos en la paleta:
  - *Terminal: nueva como administrador*: abre una shell elevada en su propia
    ventana (UAC), en la raíz del proyecto, sin tocar la IDE.
  - *NuIde: reiniciar como administrador*: relanza toda la IDE elevada —las
    terminales del panel incluidas—. Con cambios sin guardar pasa por el diálogo
    de confirmación (guardar/descartar/cancelar).
- Un proceso no elevado **no puede** meter un proceso elevado en su propio
  ConPTY: por eso la terminal embebida sólo es admin si lo es toda la IDE.
- El pie muestra **ADMIN** cuando la app corre elevada.

### 3.36 Menú contextual del coder con los comandos de NuCoder

- **Clic derecho** en el **panel del agente (coder)** suma a Copiar/Pegar los
  **atajos de los comandos** de la TUI de NuCoder, agrupados:
  - **tareas** (`/complete`, `/refactor`, `/debug`, `/explain`, `/test`,
    `/architect`): se **insertan** en la entrada (comando + espacio, sin Enter)
    para escribir el pedido;
  - **acciones** (`/free`, `/clear`, `/help`, `/quit`): se **ejecutan** al toque
    (comando + Enter).
- La terminal local conserva **Copiar/Pegar/Elevar a administrador**; el coder no
  ofrece elevar. Los ids internos usan los prefijos `agent.insert:`/`agent.run:`.

### 3.37 Hilos y estados: épocas por tarea y tope de drenado

- **Épocas (`gen`)** en los resultados de **build**, **git**, **sqlite** y **lint**
  (mismo patrón que `search_gen`): cada resultado lleva la época con que se lanzó y
  en `drain` se **descarta si no es el actual**. Evita que un resultado viejo pise
  el estado nuevo: `git_busy` sólo lo libera la operación más reciente, la salida y
  los diagnósticos de build son los del último build, el lint vale sólo si es el
  más reciente y el archivo sigue activo, y SQLite sólo aplica la última lectura.
- **Tope de drenado por tick** (`drain`): la salida de los **PTY** (local y agente)
  se procesa con un presupuesto de 512 mensajes por tick; el resto queda para el
  próximo, para que un flood de salida no clave un frame. Los demás canales emiten
  pocos mensajes y no se acotan.
- **Cancelación cooperativa del build**: cada corrida lleva un `Arc<AtomicBool>`;
  al relanzar un build se cancela el anterior (se comprueba entre pasos y aborta
  con `[build cancelado]`). El despliegue ya estaba serializado por `deploy_busy`
  (sin solapamiento), así que no necesita lo mismo.
- **Explorador fuera del hilo de UI**: `explorer::build_tree` es una función libre
  que corre en un hilo; el resultado `(época, nodos)` se aplica en `drain` sólo si
  es el más reciente. Los toggles/filtro/cambio de raíz usan métodos de estado sin
  reconstrucción (`toggle_expand`, `set_show_generated_state`, `set_root_state`) y
  luego `explorer_spawn`, así el recorrido del FS no clava la UI (repos grandes).

### 3.38 NuCoder nativo en NuIde (Slint) + `run_in_terminal` + auto-aprobar

- **Núcleo compartido.** `nucoder` pasó a ser **lib + bin** (`src/lib.rs`,
  UI-agnóstico). El CLI conserva `ask`/`provider`/`models`/`discover`/`route` y
  suma `config`; la antigua TUI (`ratatui`/`crossterm`, `tui.rs`/`theme.rs`) se
  eliminó. `markdown.rs` se reescribió como **segmentador UI-agnóstico** (prosa +
  bloques de código tokenizados).
- **Panel nativo.** `ui/ide_nucoder.slint` dibuja el chat en el dock del agente
  (rol, Markdown del asistente, bloques de código con resaltado, barra de
  confirmación, switches). `src/nucoder_ui.rs` corre el agente en un hilo y
  traduce `AgentEvent` a `IdeChatMsg`; `Ide::drain` lo drena junto al resto.
  `Lanzar NuCoder` ya **no** inyecta el binario en el PTY: abre el panel.
- **Auto-aprobar.** Nuevo `auto_approve` en la config (pisa a `confirm`):
  switch en la cabecera del panel (persistido) y `nucoder config auto-approve
  on|off` por CLI.
- **`run_in_terminal`.** El agente puede pedir que un comando se **escriba en una
  terminal real de la IDE** (proceso largo/interactivo: un web server). Viaja por
  un canal de `TerminalJob`; NuIde lo escribe en la terminal local y le contesta
  al modelo. Es sensible: respeta confirmación/auto-aprobar.
- **Enlace.** NuIde depende de `nucoder` por *path* (`exclude = ["nucoder"]` en su
  workspace); el `.exe` enlaza las dos libs SpASM (`nuide_*` y `nucoder_*`, sin
  colisión porque el C generado es `static`).

### 3.39 Panel de NuCoder: paridad con la TUI

- **Modelo y tarea**: `Config::curated_models()` (núcleo) alimenta un selector en
  la cabecera del panel; la tarea cicla desde la barra. Se persiste `default_model`.
- **Imágenes (visión)**: chip `imagen` (diálogo nativo de archivos) + chip `N img ✕`
  para quitar; el turno se rutea a modelos con visión.
- **Contexto**: chip `ctx` que cicla 16k/32k/48k/96k/128k y lo persiste.
- **Atajos globales con el foco en el chat**: `IdeNucoder` es un `FocusScope` que
  reenvía a Rust las teclas no consumidas (`global-key` → `Ide::route_global_key`,
  la misma cadena de interceptores que las terminales). Mientras hay un modal
  (paleta/búsqueda/prompt), el `TextInput` se deshabilita (`keyboard_modal_open`)
  para que el teclado vaya al modal.
- **`run_in_terminal`** elige la terminal por nombre (`local_id_by_name`).
- **Pulido**: chrome de panel, cabecera 10px/1.6, controles `NChip`/`NToggle`/
  `NButton` (misma métrica que `GitButton`), verificado con renders de
  `slint-viewer --screenshot`.

### 3.40 NuCoder: permisos + plan/autopiloto

- **Permisos** (`nucoder::config`): `permissions { web, read_outside, terminal }`
  + `autopilot`. Se aplican en `tools::Ctx` (`web`, `read_outside`,
  `terminal_allowed`): internet se bloquea, leer fuera del proyecto se habilita
  (las escrituras siguen confinadas) y `run_in_terminal` respeta su permiso.
- **Pestaña Permisos** en el panel (`PermRow` + `NToggle`): cuatro interruptores
  persistidos; se abre con el chip **permisos** de la cabecera.
- **Plan/autopiloto**: nueva tool `submit_plan` (objetivo + pasos). El bucle del
  agente la intercepta y emite `AgentEvent::Plan`; el panel muestra el plan y
  pide *Aprobar*/*Rechazar* (siempre explícito, incluso con auto-aprobar). Tras
  aprobar, con `autopilot` el resto del turno corre sin confirmar
  (`auto = true`). El prompt de sistema guía a planificar y a no frenar en
  autopiloto.

### 3.41 Cliente FTP (Opción B: motor curl)

- **Motor** `src/ftp.rs`: `Op`, parser de `LIST` (Unix `ls -l` + MS-DOS, tolerante),
  `normalize_remote`, argv por operación, **credenciales por `curl -K`** (no en
  `argv`), `spawn_op`/`run_job` en hilo + tests.
- **FTPS corregido**: explícito por defecto (`ftp://` + `--ssl-reqd`, puerto 21);
  implícito sólo en el puerto 990 (`Site::url_for`/`is_explicit_ftps`).
- **Modal** `ui/ide_ftp.slint` + módulo `module.ftp` (Preferencias → Módulos) +
  comando de paleta *«FTP: abrir cliente»*: conectar (o traer el sitio del
  proyecto), listar, navegar (↑/⟳), subir/bajar, borrar (confirmación inline) y
  nueva carpeta. Estado en `Ide` (`ftp_*`) con canal `ftp_tx`/`ftp_rx`.
- **NuCoder lo controla**: permiso **FTP** (`permissions.ftp`, `Config::perm_ftp`)
  en la pestaña Permisos + tools `ftp_open`/`ftp_list`/`ftp_upload`/`ftp_download`/
  `ftp_delete`/`ftp_mkdir` vía `FtpJob` (patrón `TerminalJob`); el IDE las ejecuta
  con `curl` y responde al modelo. `ftp_delete`/`ftp_upload` son sensibles
  (respetan confirmación / auto-aprobar).
- `docs/FTP_CLIENT.md` documenta el plan, la **evaluación de crates** y la de la
  doc de curl y de PowerShell.

### 3.42 Panel de NuCoder: proceso oculto, copiar y final + procesos sin consola

- **🧠 «pensando»**: los mensajes de **proceso** (herramientas/avisos) se ocultan
  por defecto; un icono 🧠 en la cabecera **titila** mientras el agente trabaja
  (pulso que alterna el timer de la IDE) y, al hacer clic, muestra/oculta el
  proceso.
- **Copiar**: botón **«copiar»** por respuesta (texto crudo del mensaje) y por
  **bloque de código** (sólo ese código); van al portapapeles del sistema
  (`clipboard_write`). `IdeChatMsg` ganó `text` y `IdeChatSeg` ganó `raw`.
- **«final»**: botón en el pie que baja el chat al final del texto.
- **Sin ventanas de consola**: nuevo `src/proc.rs` (`hidden()` con
  `CREATE_NO_WINDOW`) usado por `curl`/`scp`/`git`/`python`/`cc`/`sqlite3`/el
  `cmd /C start` y el script de despliegue, para que no parpadee una consola al
  correr herramientas externas desde el binario GUI.

### 3.43 Ajustes de UX: Ejecutar/Compilar con feedback, stick y FTP

- **Ejecutar/Compilar** (barra superior): estaban bien cableados pero **sin
  feedback** cuando no había archivo activo o faltaba el toolchain (y la salida
  iba a una terminal que podía estar oculta). Nuevos `reveal_terminal()` y
  `build_msg()`: siempre se muestra el mensaje en la sesión `build` y se revela
  la terminal antes de compilar.
- **Stick del scroll** (panel de NuCoder): sólo sigue el final si el usuario ya
  estaba al final (`stick`); si sube a leer, las respuestas nuevas no lo bajan
  solas. El botón **«final»** vuelve al final.
- **Listado FTP**: entrada **`..`** para subir (sin botón de borrar), orden
  **carpetas primero** y por nombre, y **filtro** por nombre en la barra.

### 3.44 `nucoder refresh` (re-bajar y reclasificar sin perder overrides)

- `nucoder refresh [id] [--reset-free]`: vuelve a bajar el catálogo del/los
  proveedor(es) y **fusiona** con los modelos guardados (`config::merge_discovered`):
  conserva el `free` fijado a mano y el costo; recalcula nombre/potencia/caps.
  Con `--reset-free` el `free` se recalcula desde el filtro del proveedor.
- Con test (`merge_conserva_free_manual_salvo_reset`).

### 3.45 Completado semántico de SpASM en el editor

- **Ctrl+Espacio** abre el completado sobre la palabra bajo el cursor: primero los
  **símbolos del AST** (`self.outline`, del parser real de SpASM) y después las
  **palabras clave/builtins** (`highlight::spasm_words`). `↑`/`↓` navegan,
  `Enter`/`Tab` insertan (reemplazan el prefijo), `Esc` cierra; tipear/Backspace
  actualizan el prefijo.
- Sólo para archivos **SpASM**. Popup anclado a la línea activa
  (`ui/ide_editor.slint`). Con test (`el_completado_usa_los_simbolos_del_ast`).

### 3.46 NuCoder: endpoints no-chat (imagen/audio)

- `client::image_generate` (`POST /images/generations`, `b64_json` o `url`) y
  `client::transcribe` (`POST /audio/transcriptions`, multipart). Base64 de
  decodificación propio en `image.rs` (`base64_decode`).
- Tools `image_generate` (guarda el PNG en `<proyecto>/generated/`) y
  `audio_transcribe`; eligen el modelo por **capacidad** (`pick_with_cap` sobre
  `Config::candidates`) y respetan el **permiso de internet**.
- `Ctx` ganó `config` para poder elegir proveedor/modelo por capacidad.

### 3.47 Sin ventanas de consola también en NuCoder

- El arreglo de §3.42 cubría los procesos de **NuIde**; faltaban los de
  **NuCoder** (que corre dentro de NuIde y lanzaba `curl` con `Command::new`).
- Nuevo `nucoder/src/proc.rs` (`hidden()` + `curl()`, `CREATE_NO_WINDOW`) y
  reemplazo en `client.rs` (chat/stream, `models`, imagen, audio, descarga) y en
  `tools.rs` (`bash`, `web_fetch`/`web_search`). Se eliminó `client::curl_bin`.

### 3.48 Chat de NuCoder: tarjetas con avatares

- Cada mensaje va en una **tarjeta** con borde redondeado (`radius-sm`,
  `border-faint`), fondo distinto para el usuario (`control`) y la IA (`panel`).
- **Avatares** por rol (usuario 🧑, IA 🤖, herramienta 🔧, aviso •, error ⚠) con
  fondo por rol, para diferenciar quién habla.
- Se **redujo el espacio** entre mensajes (spacing 12 → 6) y se quitó el punto de
  rol (lo reemplaza el avatar). Ajuste del bloque de código para no entrar en
  bucle de binding dentro del layout.

### 3.49 NuCoder: pestaña «modelos» (filtro, especialidad y ping)

- Nueva pestaña **modelos** en el panel de NuCoder: **filtro** por nombre o
  especialidad, y cada fila muestra la **especialidad** (`coder`, `visión`,
  `imagen`, `voz/audio`, `video`, `embedding`, `guard` — de las capacidades),
  gratis/poder, el **estado de ping** y botones **probar**/**usar**.
- **Ping** por modelo (`client::ping`: `POST /chat/completions` con
  `max_tokens: 1`, mide ms) y **probar todos**; verde/rojo según responda.
- `caps::specialty` (etiquetas legibles) y `Cap::label`. «usar» sólo en modelos
  de chat (`text`), para forzar el modelo.
- **Agrupado por especialidad** en secciones (orden `coder → visión → imagen →
  voz/audio → video → embedding → guard → texto`), gratis primero dentro de cada
  grupo; el filtro se sigue aplicando y los índices de «probar»/«usar» cuentan
  las cabeceras de sección.

### 3.50 NuCoder: ordenar y sincronizar el listado de modelos

- **Orden** en la pestaña modelos (chip «orden: …» que cicla): `especialidad`
  (agrupado), `ping` (más rápidos primero), `nombre` y `gratis`.
- **Sincronizar**: botón que re-baja el catálogo de todos los proveedores en un
  hilo y **actualiza la lista** (conserva los `free` a mano). La lógica de
  refresh se movió al núcleo: `config::refresh_catalog(cfg, only, reset_free)` la
  comparten el CLI (`nucoder refresh`) y el panel.

### 3.51 Guardián: las API keys no van al repo

- Las keys (y las contraseñas de sitio) viven **fuera del repo**:
  `~/.config/nucoder/config.json` y `%APPDATA%\NuIde`. (Decisión ya documentada.)
- **Test guardián** `providers::el_repo_no_tiene_keys_versionadas`: recorre los
  archivos versionados (`git ls-files`) y falla si alguno tiene algo con forma de
  key (`sk-`, `gsk_`, `nvapi-`, `AIza`, `xai-` + ≥16 alfanuméricos). Se saltea si
  no hay `git`. Con test del detector (`el_detector_reconoce_keys_y_ignora_ejemplos`).
- `.gitignore`: bloque de seguridad (`.env*`, `*.pem`, `*.key`, `secrets*.json`).

### 3.52 Skills de Claude/OpenCode/nativos en NuCoder (fases S0–S1)

- **Núcleo** `nucoder/src/skills.rs`: `Registry` que descubre y normaliza los
  tres formatos —**Claude** (`SKILL.md` con frontmatter YAML), **OpenCode**
  (`SKILL.md` o `<id>.md`) y **nativo SpASM** (`skill.toml` + `README.md`)—
  con **precedencia por id** (nativo → `.claude` → `.agents` → `opencode`
  global → `.opencode` raíz→cwd → rutas del config/`NUCODER_SKILLS`).
  Parsers propios y tolerantes (sin crates nuevas): frontmatter YAML mínimo
  (`name`, `description`, `allowed-tools`, `disable-model-invocation`,
  `metadata.opencode/autoinvoke`, `metadata.version`) y `skill.toml` mínimo
  (`name`, `version`, `entrypoint_doc`, `capabilities`, `notes`).
- **Divulgación progresiva**: al modelo sólo se le anuncia `id` + descripción
  (sección **SKILLS** del prompt de sistema, vía `Registry::catalog_prompt`);
  el cuerpo se carga **a demanda** con la nueva tool **`skill`** (por id), que
  devuelve el cuerpo sin frontmatter + el directorio base + hasta 10 archivos de
  apoyo. `autoinvoke: false`/sin descripción ⇒ no se anuncia; igual se carga.
- **Sandbox**: los directorios de skills se suman como raíces de **lectura**
  permitidas (`Ctx.allows`), así el modelo puede leer sus `references/` aunque
  vivan fuera del proyecto (`~/.claude`, `.agents`, …), sin habilitar escritura.
- **Config/CLI**: campo `skills` en la config propia; `nucoder skills` (lista
  con origen y capacidades), `nucoder skill <id>` (imprime el cuerpo) y
  `--skill <id>` (precarga el skill en el system del `ask`).
- **Panel de NuIde**: el chat arma el `Registry` al abrir/cambiar de proyecto y
  lo pasa al `Agent`; el catálogo entra al prompt de sistema.
- **Permisos (S2)**: `skills_permissions` en la config (`[{id (glob), effect}]`,
  la **última** regla que matchea gana; efectos `allow`/`ask`/`deny`) y el toggle
  global `skills_autoinvoke`. `deny` oculta del catálogo **y** hace fallar
  `load`; `ask` se anuncia pero **cargarlo pide confirmación** (lo trata como
  acción sensible el bucle del agente, respetando `confirm`/auto-aprobar). CLI:
  `nucoder config skills-autoinvoke on|off`, y `nucoder skills` muestra el efecto
  de cada uno.
- **Catálogo en la IDE (S3)**: modal `ui/ide_skills.slint` (lista con id, origen
  y estado —anunciado/oculto/ask/deny—, panel de detalle con el cuerpo y botones
  **Anunciar/ocultar** y **Refrescar**). Se abre con la paleta (*Skills: abrir
  catálogo*) o el menú **Agente** (*Catálogo de skills…*); apagar prende/apaga el
  anuncio del seleccionado (`allow` ↔ `deny`) y lo persiste en la config de
  NuCoder. El struct `IdeSkillRow` vive en `ui/ide_types.slint`; el estado y el
  `Registry` en `Ide` (`src/editor.rs`), que relee la config de NuCoder al abrir
  y al cambiar de proyecto. Sólo se refresca si está abierto (refresco acotado).
- **Menciones `@skill` (S4)**: `Registry::expand_mentions` detecta `@<id>` en el
  turno, **saca la mención** del texto y adjunta el cuerpo del skill al mensaje
  (sin esperar la tool). Un id inexistente o en `deny` no se carga (se avisa).
  Funciona en el panel y en `nucoder ask`.
- **REA (M0–M1)**: se verificó **REA 6.4.0** con Node 26; su skill
  `reverse-engineer-anything` se instaló en `~/.agents/skills/` (con
  `references/`) y NuCoder lo descubre y lo anuncia. Falta el cliente MCP (M2)
  para sus herramientas.
- Tests nuevos: **18** en `nucoder` (skills/tools) y **1** en `nuide` (comando
  de paleta del catálogo).
  (Plan completo: `docs/SKILLS.md`; MCP y REA: `docs/MCP.md`.)

### 3.53 Cliente MCP en NuCoder (integración con REA)

- **`nucoder/src/mcp.rs`**: cliente **MCP** (Model Context Protocol) — JSON-RPC
  2.0 sobre **stdio**. Lanza el server como proceso hijo (sin consola), hace
  `initialize` → `notifications/initialized` → `tools/list`, y llama por
  `tools/call`. Hilo lector que manda los mensajes por canal; las peticiones
  esperan el `id` que les corresponde (handshake 30 s, llamada 300 s).
  `shutdown`/`Drop` matan el proceso. Sin crates nuevas.
- **Config `mcp.servers`** en `~/.config/nucoder/config.json`: `command`, `args`,
  `env`, **`tools`** (filtro glob de qué exponer; vacío = todas), `enabled` y
  `confirm`. Se parsea/persiste con `McpServer::from_value`/`to_value`.
- **Agente**: al empezar el turno lanza los servers habilitados, **suma sus
  herramientas al modelo** como `<id>__<tool>` (p. ej. `rea__open_binary`),
  enruta las llamadas a `tools/call`, las trata como **sensibles** si
  `confirm` (Aprobar/Rechazar, lo saltea auto-aprobar) y **apaga** los procesos
  al terminar. El **filtro `tools`** evita volcar cientos de specs al prompt.
- **CLI**: `nucoder mcp list`, `nucoder mcp tools <id>`,
  `nucoder mcp call <id> <tool> [json]`.
- **Verificado con [REA](https://github.com/morluto/rea) 6.4.0**: se levantó su
  MCP (`cmd /d /c npx -y rea-agents@6.4.0 mcp`) y NuCoder listó **170
  herramientas**.
- Tests nuevos (**6**): parseo/round-trip de la config MCP (2), separación del
  prefijo `<id>__<tool>` (1) y e2e contra el fixture `src/bin/mcp_stub.rs` (3:
  handshake+lista+llamada, `isError`, comando inexistente). El binario `mcp_stub`
  es **sólo un fixture de tests** (`default-run = "nucoder"`).
- **Pendiente (M2)**: UI **Preferencias → MCP** (CRUD, probar, ver tools). La
  conexión ya es **persistente** (`McpHub`): los procesos siguen vivos entre
  turnos, se reconectan si cambia la config o el proyecto, y se apagan al cerrar
  el panel/app.

### 3.54 NuCook: destilar una habilidad (sin ruido humano)

- **`nucoder/src/cook.rs`**: convierte un skill (Claude/OpenCode/nativo) en su
  **forma enseñable** —once relaciones, una por línea: `when`, `not`, `want`,
  `have`, `do`, `keep`, `map`, `prove`, `eg`, `risk`, `rest`— con render **texto**
  (para el modelo) y **JSON** (Rust/UI/caché). Determinista, sin modelo.
- **Criterio**: "sin pérdida" no es igualdad de bytes (eso conservaría el ruido)
  sino que **la habilidad siga siendo enseñable**. El ruido humano tiene dos
  caras: **de más** (relleno/formato) y **de menos** (supuestos implícitos).
- **Dos guardianes** (tests): **(1) no se pierde** (`unidades == conservadas +
  ruido + duplicados`; lo dudoso va a `rest`, no a la basura) y **(2) no hay
  ruido** (sin reglas, encabezados, boilerplate de licencia/versión ni
  duplicados).
- **CLI**: `nucoder cook <id|ruta> [--json] [--stats]`; `Skill::body()` y
  `skills::strip_frontmatter` en el núcleo.
- **Gourmet (C3)**: `nucoder cook <id> --gourmet [--model <id>] [--refresh]` hace
  un pase **semántico** con el modelo (`client::complete`, una llamada one-shot)
  para atacar el ruido *de menos* (supuestos, prosa). El prompt define cada
  etiqueta y trae un *few-shot*; la salida se parsea a la misma forma. Caché en
  `~/.config/nucoder/cookies/` con clave `hash` + modelo + **versión del prompt**
  (si cambia el prompt, se invalida). Probado contra REA: pasó de fragmentos por
  línea a `when/not/want/have/do/keep/map` correctos.
- **Binario `nucook` + recetario**: la CLI de NuCook como ejecutable propio
  (`nucoder/src/bin/nucook.rs`): `nucook <id>`, `nucook list`, `nucook bake`
  (cocina **todas** y guarda el **recetario** en
  `~/.config/nucoder/cookbook/<id>.cookie`, JSON) y `nucook show <id>`.
  Probado: `nucook bake` cocinó las 10 recetas descubiertas.
- **Cocinar docs/KB/datasets (C4 parcial)**: `nucook cook-dir <dir> [--gourmet]
  [--out <dir>]` recorre los `.md`/`.markdown`/`.txt` (`cook::collect_docs`,
  salteando generados/ocultos) y guarda una receta por documento; sirve para la
  **KB**, docs y datasets. Probado: 2 docs → 2 cookies.
- **Límite honesto**: el pase determinista fragmenta por línea en skills con
  mucha prosa; el gourmet rinde ahí. Ver [`docs/NUCOOK.md`](../../docs/NUCOOK.md).

### 3.55 C2: el agente come las galletitas (primer + receta)

- **Catálogo enriquecido**: `cook::catalog_prompt(&Registry)` reemplaza al
  catálogo por descripción: si hay una **receta** (`nucook bake`, en
  `~/.config/nucoder/cookbook/`), anuncia su **primer** (`want … · when …`); si
  no, cae a la descripción. Es lo que ve el prompt de sistema.
- **Cargar la receta**: la tool `skill` gana `format` (`auto`/`cooked`/`raw`):
  por defecto devuelve la **receta cocida** si existe (si no, el original);
  `cooked` fuerza la cocción determinista; `raw` el cuerpo original.
- `cook::load_cookie`/`teaching_from_json` reconstruyen la receta desde el JSON.

### 3.56 Hub de skills: buscar / instalar / desinstalar

- **`nucoder/src/hub.rs`**: `search` (filtra el catálogo por id/nombre/
  descripción), `install_from_dir` (copia local) e `install_from_git`
  (`owner/repo` o URL → `git clone --depth 1` → encuentra `<skill>/SKILL.md` →
  copia) hacia el destino compartido **`~/.agents/skills`** (que `Registry` ya
  escanea), y `uninstall` (borra sólo dentro del hub, id validado).
- **CLI `nucook`**: `search`, `install <repo|ruta> [--skill <id>] [--force]`,
  `uninstall <id>`, además de `list`/`catalog`/`bake`/`show`/`cook`.
- **Búsqueda remota (catálogo)**: `hub::search_catalog(url, q)` baja un
  `index.json` (`{ "skills": [ {name, description, source|repo, path|skill} ] }`)
  con `curl`, lo parsea (tolerante a BOM) y filtra; CLI
  `nucook search <t> --remote --catalog <url>` (o `NUCOOK_CATALOG`). La UI todavía
  busca local; el toggle “buscar en la web” queda pendiente.
- **Actualizar**: “Instalar…” del modal reemplaza (force) si ya existe; también
  `nucook install … --force`.
- **Seguridad**: instalar **no ejecuta** nada; al usar el agente el skill pasa por
  confirmación y por `allow/ask/deny`.
- **Interfaz** (buscar/descargar/administrar en la UI): diseñada en
  [`docs/SKILLS_UI.md`](../../docs/SKILLS_UI.md) (fases U0–U3; U0 —el modal— hecha).
- Tests nuevos (**6**, con el recetario): guardar/reconstruir receta, primer,
  búsqueda y instalar/desinstalar en directorios temporales.

### 3.57 UI de skills: buscar, instalar, cocinar y eliminar en el modal

- **Modal `ui/ide_skills.slint` ampliado**: **buscador en vivo** (el teclado lo
  maneja Rust, como los demás modales), barra de acciones **Instalar… · Cocinar ·
  Eliminar · Anunciar · Refrescar**, indicador de **ocupado/último mensaje**, y
  el **detalle** ahora muestra la **receta** cocida si existe (si no, el cuerpo).
- **Cableado en `src/editor.rs`**: `skills_filter`/`skills_note`/`skills_busy`,
  canal `skills_tx/rx` (operaciones en **hilo**), `skills_intercept` para el
  buscador, y callbacks `skills-install/cook/delete`.
- **Instalar** reusa el diálogo `IdePrompt` (`PromptKind::SkillInstall`):
  `owner/repo`, URL o ruta, con `#skill` opcional → `hub::install_*` en hilo.
- **Cocinar** corre el pase gourmet (cacheado) sobre el seleccionado y guarda la
  receta. **Eliminar** pide confirmación (`Ask::UninstallSkill`) y usa
  `hub::uninstall` (sólo si está dentro del hub).
- **Buscar en la web (U4)**: toggle **`web`** en el modal; consulta el catálogo
  remoto (`NUCOOK_CATALOG`) en un hilo y agrega las entradas como filas (estado
  `web`); **Instalar** las baja de su repo/path.
- i18n regenerado (`tools/i18n_extract.py` + `i18n_po.py`): 441 claves.
- Diseño y fases: [`docs/SKILLS_UI.md`](../../docs/SKILLS_UI.md) (U0–U4 ✅).

### 3.58 Empaquetado de `nucook.exe` (NuCook viaja con NuIde)

- **`packaging/NuIde.iss`**: el instalador copia **`nucook.exe`** a `{app}` (bajo
  la tarea `nucoder`, junto a `nucoder.exe`); el `-AddPath` de
  `postinstall.ps1` ya deja `{app}` en el PATH del usuario, así `nucook` queda
  disponible en cualquier terminal. Textos de tareas/mensajes actualizados.
- **`packaging/make_installer.ps1`**: compila explícitamente `--bin nucook`
  (release) además de NuCoder y verifica que exista `nucook.exe`.
- **Probado**: `make_installer.ps1 -SkipBuild` generó
  `releases/NuIde-0.3.2-setup.exe` (7,8 MB) **incluyendo `nucook.exe`**.

### 3.59 Normalizador de NuCook en SpASM (AOT → C → FFI)

- **`nucoder/spasm/cook.spasm`** (dialecto AOT) decide la parte **estructural**
  de la cocción: `label_kind(s)` → índice de la etiqueta **canónica** (0..10) o
  -1, y `is_rule(s)` → 1 si es regla horizontal exacta (`---`, `***`, …). Sólo
  usa builtins documentados (`str_equals`).
- **`nucoder/ffi/cook_ffi.c`**: exporta `nucoder_cook_label_kind` /
  `nucoder_cook_is_rule`; el C generado se commitea (`spasm/generated/cook.c`,
  regenerado con `tools/regen_spasm.ps1` — el toolchain reproduce `router.c`/
  `codeassist.c` byte a byte).
- **`src/ffi.rs`** suma los bindings (mutex + `CString`); **`src/cook.rs`** usa
  SpASM para las etiquetas canónicas (los sinónimos quedan en Rust) y para
  `is_rule` (con fallback). La lógica con *case folding* Unicode sigue en Rust
  porque los builtins de string documentados no la ofrecen (regla de la KB: no
  inventar builtins).
- Test nuevo: SpASM resuelve `want`→2/`rest`→10; los sinónimos (`goal`) los cubre
  Rust. Docs: `nucoder/README.md` (tabla FFI), `docs/NUCOOK.md`.
- **Cocinar docs desde la UI**: comando de paleta *NuCook: cocinar carpeta de
  docs…* (diálogo nativo, en hilo) que cocina los `.md`/`.txt` de una carpeta al
  recetario; el resumen se ve en el modal de skills y en la terminal `build`.

### 3.60 NuCoder: anti-cuelgue del chat y modelos que no funcionan

- **Watchdog de inactividad del chat** (`nucoder/src/client.rs`): la config de
  `curl` del turno suma `speed-limit`/`speed-time` (`CHAT_STALL_SECS = 60`). Si el
  proveedor deja de mandar bytes, `curl` aborta (exit 28) y el error llega como
  `Network`, en vez de bloquear el `read_until` hasta el `max-time` (600 s) —que
  era lo que se veía como «se quedó pensando»—.
- **El agente rota ante más fallos** (`nucoder/src/agent.rs`): además de
  cuota/401/5xx/404, ahora rota con errores de **red/timeout** y de **parseo**, y
  trata una **respuesta vacía** (200 sin texto ni tool calls) como fallo en vez de
  cerrar el turno en silencio. La cancelación del usuario corta sin rotar, y un
  fallo local de `curl` no rota ni marca modelos.
- **Estado aprendido persistido** (`nucoder/src/config.rs`): `model_dead` /
  `model_ok` + `mark_dead`/`mark_ok`/`clear_status`. `Config::candidates` **saltea
  los muertos**, así el router no vuelve a elegir un modelo roto. El agente avisa
  por el evento **`AgentEvent::Dead(id)`** sólo ante fallos **del modelo** (404,
  timeout/trabado, respuesta inválida o vacía); los 5xx y los 4xx de pedido rotan
  sin blacklistear (pueden ser transitorios). El ping también marca muertos. El
  panel guarda el estado (una vez por drenado, no por mensaje).
- **Panel** (`src/nucoder_ui.rs` + `ui/ide_nucoder.slint`): chip **«solo ok»**
  (muestra sólo los que respondieron al ping) y botón **«limpiar»** para borrar el
  estado aprendido; la chip **«gratis»** ahora **persiste** y además **filtra el
  listado**; **probar todos** prueba sólo lo visible (respeta los filtros) y sus
  resultados se persisten. Al abrir, el listado ya muestra el estado conocido
  (verde/rojo) sin re-probar.
- **Gratis por defecto**: `Config::default().free_only = true` y `load()` usa
  `unwrap_or(true)`.
- Tests nuevos (4): `candidates` saltea muertos, `mark_dead`/`mark_ok`/`clear`
  se excluyen, gratis por defecto, y el watchdog en la config de `curl`.
- **Barra con íconos** (`ui/ide_nucoder.slint`): nuevo componente `NIcon`
  (glifo + tooltip + estado activo). El encabezado y la barra pasan a íconos
  (📋 modelos · 🔒 permisos · 🎯 tarea · 📎/✖ imagen · 📏 contexto · 🆓 gratis ·
  ⚡ auto-aprobar · ➕ nueva; en la pestaña modelos: ⇅ orden · 📡 probar ·
  🔄 sincronizar · ✓ solo ok · 🧹 limpiar), cada uno con tooltip; los que ya no
  se usaban como texto liberan ancho en paneles angostos. Glifos verificados con
  `slint-viewer --screenshot` (se descartaron ✕/⟳/↻ por no renderizar).
  i18n regenerado (440 claves: se van los rótulos de texto que pasaron a ícono).
- **Parser de `SKILL.md`: *block scalars*** (`nucoder/src/skills.rs`). El
  *frontmatter* YAML mínimo no entendía `description: >` (plegado) ni `|`
  (literal), y dejaba la descripción como `>`. Se agregó `block_scalar_kind` +
  `take_block` + `fold_block` (sirve para `description` y cualquier clave), con
  test. Con eso `caveman` y `ponytail` (de GitHub) se descubren y anuncian bien.

### 3.61 Gestor de skills: actualizar y configurar

- **Origen recordado** (`nucoder/src/hub.rs`): al instalar se escribe
  `.nucook-origin` dentro del skill con el `spec` (`owner/repo#skill`, URL o ruta
  local) — el de git guarda el `spec` del usuario, no el clon temporal. Nuevo
  `hub::origin`/`hub::save_origin` y **`hub::update(id, root)`** que re-instala
  desde el origen con `force`. Test: `recuerda_el_origen_y_actualiza`.
- **UI** (`ui/ide_skills.slint` + `src/editor.rs`): el modal suma una **fila de
  acciones** (Instalar… · **Actualizar** · Eliminar · **Permiso: …** · Cocinar ·
  web · **Catálogo…** · Refrescar). **Actualizar** re-instala el seleccionado;
  **Permiso** cicla `allow → ask → deny` (`skills_cycle_perm`, persistido en
  `skills_permissions`); **Catálogo…** abre el prompt para la URL del `index.json`
  y la guarda en el config de la IDE (`nucoder.catalog-url`, con
  `NUCOOK_CATALOG` de respaldo). El **detalle** muestra la ruta, la **versión** y
  el **origen**; la lista agrega `· vX` al nombre.
- Callbacks nuevos en `nuide_ui.slint` y cableado en `editor::attach`.
- i18n regenerado (446 claves).

### 3.62 Persistencia en SQLite

- **`nucoder/src/db.rs`**: todo lo que antes eran archivos JSON pasa a una sola
  base `~/.config/nucoder/nucoder.db` con tres tablas: `settings(key,value)` (la
  config completa bajo la clave `config`, más `schema`), `sessions(project, cwd,
  updated_at, messages)` (los **chats**) y `cookbook(id, cookie, updated_at)`.
  Abre con `PRAGMA journal_mode=WAL` y `CREATE TABLE IF NOT EXISTS`.
- **Migración automática** (`db::migrate`, una vez): importa `config.json`,
  `sessions/*.json` y `cookbook/*.cookie` y marca `schema=1`. **No borra** los
  originales (quedan de respaldo). Verificado: migró la config, 3 sesiones y 10
  recetas.
- **`config.rs`**: `save_own` escribe la fila `config` de la base y además una
  **exportación** `config.json` (legible/editable); `load_document` prefiere el
  archivo si existe (y lo **reimporta** a la base), si no usa la fila. Así editar
  el JSON a mano sigue funcionando, pero la fuente de verdad es SQLite.
- **`session.rs`**: `save`/`load`/`remove`/`list` van a la base (el `path`/`key_for`
  y `load_file` quedan para importar archivos viejos). **`cook.rs`**: `save_cookie`
  /`read_cookie`/`load_cookie` usan la base; `save_cookie_to` sigue escribiendo
  archivos cuando se pide un directorio explícito (`cook-dir --out`).
- **Dependencia**: `rusqlite` (feature `bundled`) en `nucoder`. Es la primera
  dependencia pesada del proyecto; se justifica porque implementar el formato de
  SQLite a mano no es razonable (el visor `sqlite.rs` ya evitaba eso).
- Tests nuevos (3) en `db.rs`: settings, sesiones y recetas; total **102**.
  (Pendiente opcional: el visor de SQLite podría leer nativo con `rusqlite` en vez
  de `sqlite3.exe`/python.)

## 4. Limitaciones conocidas
- **Persistencia.** La persistencia de NuCoder (config, **chats**, recetario)
  ahora vive en **SQLite** (`~/.config/nucoder/nucoder.db`), con migración desde
  los JSON viejos (que quedan de respaldo) y export de `config.json`. Lo que no
  se recuerda: las **sesiones de terminal**, el scrollback y el estado de la
  búsqueda. Del resto se recuerda el proyecto, el dock/visibilidad de los
  paneles, la geometría de la ventana, las pestañas abiertas, el filtro de
  generados y el cursor de cada archivo.
- **Despliegue**: la contraseña queda en texto plano en `%APPDATA%\NuIde\config`
  (fuera del repositorio, a propósito; migrable al almacén de Windows). `scp` no
  crea carpetas remotas, por eso SFTP usa `curl` salvo que se pida `scp`
  explícitamente. FTPS valida el certificado con el almacén del sistema o con la
  CA que se indique; no hay UI para aceptar un certificado en el primer contacto.
- **Apilado básico.** Cada región apila hasta 3 paneles repartiendo la altura en
  partes iguales (sin divisor arrastrable entre los apilados), y un panel existe
  una sola vez: no se puede duplicar para llenar más de 3 por lado.
- **La shell local no cambia de carpeta** al cambiar de proyecto, pero las
  sesiones nuevas y **reiniciar la sesión** arrancan en la raíz del proyecto (el
  agente sí cambia sin reiniciarse).
- **Búsqueda del editor.** Tiene expresiones regulares, acotar a la selección y
  buscar en todo el proyecto; falta *fuzzy* en la paleta (que hoy filtra por
  subcadena).
- **Editor sin *render* virtualizado.** Se materializa el modelo completo (tope
  20k líneas), así que un archivo enorme no entra completo; todavía no hay
  *word wrap*, plegado ni multi-cursor.
- **Eliminar no usa la papelera.** El explorador borra de forma permanente (con
  confirmación previa); mandar a la papelera requeriría una API del SO.
- **Diagnóstico acotado.** Corre el parser al abrir/guardar/`F6` (no en cada
  tecla), reporta sólo el **primer** error, no hay subrayado por columna ni
  panel de problemas, y el parser es el del **dialecto compilado** (no el de
  intérprete).
- **`terminal.rs` heredado.** La consola lineal ya no la usa la aplicación; sólo
  la ejercitan pruebas de integración del toolchain. Candidata a eliminar o
  migrar sus pruebas al PTY.
- **Sin depurador interactivo.** El intérprete de SpASM no expone paso a paso,
  breakpoints ni inspección de registros; un depurador requiere agregarlos al
  toolchain (`SpAMS-Oficial`).
- **Instalador de SpASM.** El paquete principal puede dejar `spasmc.py` sin sus
  generadores (`spasm-asm-gen.py`, `spasm-c-gen.py`), y entonces ese compilador
  falla. NuIde usa el que descubre (`NUIDE_SPASMC`/`SPASMC`); el `.iss` del
  compilador ya declara los scripts, así que un instalador reconstruido los
  incluye. En esta máquina se completó la instalación copiando los que faltaban.
- **Alternar proyectos reinicia el agente** (a propósito: el agente es una
  shell que lanza opencode al arrancar, así que al cambiar de proyecto se
  reinicia la sesión completa en el nuevo directorio).

---

## 5. Pendientes (hoja de ruta)

Ordenados por impacto para el uso diario. El **plan en fases** para llegar a
v1.0 (tamaño, archivos y criterio de terminado) está en [`PLAN.md`](PLAN.md).

> **Nota:** esta hoja de ruta quedó mayormente **hecha**; el estado vivo está en
> `PLAN.md` (fases 0–10). Lo que sigue es el detalle original de cada ítem.

### 5.1 Persistencia de estado

**Objetivo:** recordar entre ejecuciones el tamaño/posición de la ventana, las
pestañas abiertas, las sesiones de terminal y **qué paneles están visibles**
(además del proyecto, ya hecho).

**Enfoque:** ampliar `config.rs` a un pequeño *store* `clave=valor` (ya parsea
líneas) y guardar desde `editor.rs`/`main.rs` en los cambios relevantes. La
ventana Slint expone tamaño; la posición puede requerir el backend. Al arrancar,
aplicar geometría y reabrir pestañas/sesiones.

**Archivos:** `src/config.rs`, `src/editor.rs`, `src/main.rs`.

### 5.2 Git real

**Objetivo:** rama y estado reales en la barra inferior, y acciones básicas
(stage, commit, push) desde la IDE.

**Enfoque:** invocar el `git` del `PATH` con `std::process::Command` (evita sumar
`git2`/`libgit2`) y volcar el estado; menú contextual o panel para las acciones.
Un `src/git.rs` con descubrimiento + parseo de `status --porcelain`.

**Archivos:** `src/git.rs` (nuevo), `src/editor.rs`, `ui/ide_statusbar.slint`.

### 5.3 Atajos y búsqueda

**Hecho** (iteración 3.15): paleta de comandos (`Ctrl+Shift+P`) y buscador de
archivos (`Ctrl+P`) globales. En el editor ya están buscar/reemplazar
(`Ctrl+F`/`Ctrl+H`) con mayúsculas y palabra completa.

**Falta:** *fuzzy*/regex en la paleta y en la búsqueda, y acotar la búsqueda a la
selección.

**Archivos:** `ui/ide_palette.slint`, `src/editor.rs`.

### 5.4 Mouse reporting en las terminales

**Objetivo:** reenviar el mouse a los procesos que lo piden sin perder la
selección/copia.

**Enfoque:** detectar el modo de mouse que habilitó el programa (el parser
`vt100` expone los modos) y, si está activo, mandar los eventos codificados al
PTY en vez de seleccionar; reservar la selección para cuando no lo esté.

**Archivos:** `src/vt.rs`, `src/editor.rs`, `ui/ide_terminal.slint`,
`ui/ide_opencode.slint`.

### 5.5 Empaquetado

**Objetivo:** `.exe` release con icono y versión, e instalador, como las otras
apps del ecosistema.

**Enfoque:** recursos de Windows (`winres`/`build.rs`), perfil `release` y un
script de instalación.

**Resuelto (arranque sin consola):** el `.exe` de release era de subsistema de
*consola*, así que al abrirlo desde el Explorador / el menú Inicio aparecía una
terminal vacía antes de la ventana. `src/main.rs` ahora declara
`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` (ventanas en
release, consola en debug). Como `--version` / `--help` siguen necesitando
imprimir, `attach_console()` engancha la consola del padre con `AttachConsole`
**sólo si los handles estándar no existen**: si ya vienen con una redirección
(archivo, caño, logs de la CI) se respeta y no se pisa con `CONOUT$`. La escritura
es con `write_all` + `let _ =` para no entrar en panic cuando no hay nadie del
otro lado.

`packaging/make_installer.ps1` hace el ciclo completo: `cargo build --release`,
verifica el subsistema del `.exe`, busca `ISCC.exe` (no está en el `PATH` cuando
Inno Setup se instala por usuario: `%LOCALAPPDATA%\Programs\Inno Setup 6`) y
compila `NuIde.iss`. Con `-SkipBuild` sólo recompila el instalador.

---

## 6. Verificación

- `cargo build` / `cargo check` sin warnings.
- `cargo clippy --all-targets -- -D warnings` limpio.
- `cargo test`: **110 pruebas OK** (unitarias; las de PTY, el linter y el build
  lanzan `cmd`/`opencode`/Python/spasmc reales y se saltean si no están).
- `cargo fmt --check` limpio.
- `slint-viewer --check ui/nuide_ui.slint` OK.
- `nuide.exe --version` (smoke test sin display, lo usa la CI: imprime aunque el
  `.exe` sea de ventanas, y respeta la salida redirigida).
- Arranque real del `.exe` sin panics y **sin terminal vacía** (subsistema de
  ventanas en release; se verifica leyendo el campo Subsystem del encabezado PE).
- Guía de usuario en [`GUIA.md`](GUIA.md).

### Cómo probar a mano

```sh
cargo run --bin nuide
# UI sin backend (datos demo):
slint-viewer ui/nuide_ui.slint
```

Para el panel del agente hace falta `opencode` en el `PATH` (o `NUIDE_AGENT`).
Para Ejecutar/Compilar, el toolchain de SpASM (ver §"Dónde busca el toolchain"
del README).
