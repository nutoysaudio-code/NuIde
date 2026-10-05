# NuIde — estado y pendientes

**Actualizado:** 2026-10-04.
Este es el *parte de estado*: qué está hecho, qué se cambió en la última
iteración, qué limitaciones quedan y qué falta. El [README](../README.md) es la
guía de uso; la especificación visual/funcional es
[`NUIDE_IDEA.md`](NUIDE_IDEA.md).

---

## 1. Qué es NuIde

IDE de escritorio para **SpASM** (Rust + Slint): explorador de archivos, editor
con resaltado, **dos terminales reales** (una shell local y la TUI de OpenCode) y
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
  en menta cuando está visible) para las tres regiones y uno que abre el menú Ver.
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
- **Plantillas** (`src/templates.rs`): `compilado`, `interprete` y `vacio`.
  `new_file_with(body)` crea el archivo con el cuerpo de la plantilla; comandos
  de paleta por plantilla y `new_project` (crea `src/main.spasm`, fija el
  proyecto y lo abre).
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

---

## 4. Limitaciones conocidas

- **Persistencia parcial.** Se recuerda el proyecto, el dock/visibilidad de los
  paneles, la geometría de la ventana, las pestañas abiertas, el filtro de
  generados y el cursor de cada archivo. Las **sesiones de terminal, el
  scrollback** y el estado de la búsqueda no.
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
- **Alternar proyectos relanza el agente** y pierde su sesión en curso (a
  propósito: el agente trabaja sobre la raíz del proyecto).

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

---

## 6. Verificación

- `cargo build` / `cargo check` sin warnings.
- `cargo clippy --all-targets -- -D warnings` limpio.
- `cargo test`: **108 pruebas OK** (unitarias; las de PTY, el linter y el build
  lanzan `cmd`/`opencode`/Python/spasmc reales y se saltean si no están).
- `cargo fmt --check` limpio.
- `slint-viewer --check ui/nuide_ui.slint` OK.
- `nuide.exe --version` (smoke test sin display, lo usa la CI).
- Arranque real del `.exe` sin panics.
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
