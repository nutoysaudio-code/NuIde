# NuIde — IDE multi-lenguaje con SpASM, terminales y OpenCode

IDE de escritorio con soporte de **SpASM** (transpilar, compilar, ejecutar y
lint con su toolchain) **y de otros lenguajes**: explorador de archivos, editor
de código con resaltado para ~24 lenguajes, terminal local y una terminal
dedicada a **OpenCode CLI**.

NuIde es un **proyecto independiente**: una aplicación de escritorio en
**Rust + Slint** que no depende del workspace de audio ni de ningún crate
compartido de plugins. Sus dependencias son `slint` (UI), `arboard`
(portapapeles), `portable-pty` + `vt100` (terminales reales), `rfd` (diálogos
nativos) y `regex-lite` (búsqueda con expresiones regulares). Tiene su propio
`[workspace]` para compilarse solo.

La paleta es oscura y propia (instrumento electrónico sobrio): vive en
`ui/ide_theme.slint`. La especificación visual está en
[`docs/NUIDE_IDEA.md`](docs/NUIDE_IDEA.md) y el boceto original es
`SpASM Studio con terminales integrados.png` (el archivo conserva ese nombre
histórico; NuIde no es un studio exclusivo de SpASM). La **guía de uso** está en
[`docs/GUIA.md`](docs/GUIA.md).

```sh
cargo run --bin nuide          # o `cargo build --release`
```

---

## Estado

La terminal, el explorador, el editor (edición con resaltado y guardado), el
panel de **OpenCode CLI** y el **control de código (Git)** ya son **reales**. La
interfaz dejó de tener datos ilustrativos: la pantalla del editor sin archivos
abiertos es una **página de bienvenida** real (accesos y atajos).

| Fase | Contenido | Estado |
|---|---|---|
| 1 | andamiaje: crate + ventana + tema + layout de 3 columnas | ✅ |
| 2 | divisores arrastrables + scroll por área | ✅ |
| 3 | explorador: abrir/cerrar carpetas y abrir archivos reales | ✅ |
| 4 | editor: edición, guardado y resaltado real de SpASM | ✅ |
| 5 | terminal real (PTY/ConPTY) + panel de OpenCode CLI | ✅ |
| 6 | integración con el toolchain `spasm`/`spasmc` (build/run) | ✅ |
| 7 | terminal: scrollback + selección y copia con el mouse | ✅ |
| 8 | panel derecho: terminal real + opencode bajo demanda (TUI real) | ✅ |
| 9 | elegir proyecto (diálogo nativo) + recordarlo | ✅ |
| 10 | editor: buscar/reemplazar + resaltados de contexto | ✅ |
| 11 | layout modular: controles en la barra + menú Ver | ✅ |
| 12 | dock por panel + apilado por lado (hasta 3) | ✅ |
| 13 | diagnóstico real: parser de SpASM en el editor | ✅ |
| 14 | terminales: grilla exacta (ancho de celda medido) | ✅ |
| 15 | editor: resaltado por lenguaje (SpASM, Slint, Python, C, …) | ✅ |
| 16 | visores: imágenes + vista previa de Markdown | ✅ |
| 17 | build/run desde la IDE con errores navegables (`F8`) | ✅ |
| 18 | esquema (outline) + ir a definición (`F12`) | ✅ |
| 19 | terminal inferior dividida (dos vistas) | ✅ |
| 20 | página de bienvenida del editor (accesos + atajos) | ✅ |
| 21 | bienvenida: recientes (proyectos/archivos) + nuevo archivo | ✅ |
| 22 | control de código (Git): rama, stage, commit y push | ✅ |
| 23 | paleta de comandos (`Ctrl+Shift+P`) + buscar archivos (`Ctrl+P`) | ✅ |
| 24 | menús Archivo/Editar + guardar como / cerrar con confirmación | ✅ |
| 25 | explorador: nuevo/renombrar/eliminar, contextual y filtro de generados | ✅ |
| 26 | editor: regex, buscar en selección, historial, autocierre/autoindent y tab | ✅ |
| 27 | terminal: mouse reporting + renombrar/reiniciar sesión | ✅ |
| 28 | buscar y reemplazar en todo el proyecto | ✅ |
| 29 | Git avanzado: diff, ramas, pull, ahead/behind, descartar/amend | ✅ |
| 30 | flujo SpASM: config de ejecución, plantillas y ayuda de sintaxis | ✅ |
| 31 | preferencias (fuente/tamaño/acento) y atajos configurables | ✅ |
| 32 | calidad/CI + icono + instalador Inno Setup (Node/CLI) + pie CPU/RAM | ✅ |
| 33 | guía de usuario (`docs/GUIA.md`) + pulido | ✅ |
| 34 | lógica de la IDE en **SpASM** (FFI AOT): diagnósticos y config | ✅ |
| 35 | proveedores de OpenCode: CRUD sobre la config del CLI | ✅ |

> El parte de estado detallado —qué se cambió en cada iteración, limitaciones y
> pendientes con su enfoque— está en [`docs/ESTADO.md`](docs/ESTADO.md).

### Lógica de la IDE en SpASM (FFI AOT)

Funciones centrales de la IDE escritas en SpASM (dialecto **compilado AOT**,
`programa` + `main {}`) y compiladas a C en cada build vía `cc`:

- `spasm/diagnostics.spasm` — `parse_diagnostics` / `diagnostic_of` (usa el
  backend C real de `src/lint.rs`); los detalles salen por la global `diag_msg`.
- `spasm/configkv.spasm` — `config_parse` (líneas `clave=valor`, con `#` como
  comentario); el valor de la última clave sale por la global `cfg_out`.
- `spasm/generated/*.c` — C generado commiteado (clone limpio compila sin
  Python); `tools/regen_spasm.ps1` lo refresca (test `el_c_generado_esta_al_dia`).
- `src/spasm_logic.rs` — wrappers Rust seguros (mutex global, buffers ≤4096) y
  tests de paridad; contrato FFI en `spasm/README.md`.

### El explorador (real)

`src/explorer.rs` arma un árbol **plano con profundidad**, que es lo que dibuja
`IdeExplorer`, y guarda el `path` real de cada fila. La raíz es la carpeta del
proyecto (la última elegida o la de lanzamiento). Hay topes de 500 entradas por
carpeta y 4000 nodos para que un `target/` o un `node_modules/` no cuelguen la
UI.

- Clic en una carpeta: se abre o se cierra.
- Clic en un archivo: se abre en una pestaña, se resalta y queda como archivo
  activo para los menús Ejecutar/Compilar.
- Las extensiones de configuración (`.toml`, `.json`, `.yaml`, …) llevan el
  icono de engranaje.
- **Clic derecho** abre el menú contextual: Nuevo archivo/carpeta, Renombrar,
  Eliminar, Copiar ruta, Mostrar en el explorador y Refrescar. Nuevo/renombrar
  usan el diálogo de entrada (`ui/ide_prompt.slint`); Eliminar pide confirmación
  (`IdeAsk`) y cierra las pestañas de lo borrado. Renombrar reapunta las
  pestañas abiertas.
- En el encabezado, **⟳** refresca y **◍** oculta/muestra los generados
  (`target`, `node_modules`, `dist`, `.git`, …); el filtro se recuerda.

La **raíz** es la carpeta del proyecto: la última elegida o la carpeta de
lanzamiento. Se cambia con **clic en el nombre del proyecto** en la barra
superior, que abre el diálogo nativo de carpetas (`rfd`). Al elegir otra, el
explorador se reconstruye, el **panel derecho se reinicia** en la carpeta nueva
(como terminal; opencode se lanza bajo demanda) y la elección se recuerda en
`%APPDATA%\NuIde\config` para el próximo arranque.

### El editor (edición + resaltado)

`src/highlight.rs` detecta el **lenguaje por la extensión** y resalta cada tipo de
documento con su tokenizador: **SpASM**, **Slint**, **JSON**, **TOML**, **YAML**,
**Markdown**, **shell**, **Rust**, **Python**, **JavaScript**, **TypeScript**,
**C**, **C++**, **C#**, **Java**, **Go**, **Kotlin**, **Swift**, **Lua**, **Ruby**,
**PHP**, **HTML**, **XML**, **CSS** y **SQL** (y texto plano para el resto). La
**barra de estado** muestra el lenguaje real del archivo activo. Los tabs se
expanden a cuatro espacios y la concatenación de tokens reproduce la línea
exacta, que es lo que necesita el render monoespaciado.

El buffer es un `Vec<String>` por pestaña, con cursor por **caracteres** (no
bytes, para no romper acentos) y flag de cambios sin guardar (el punto ámbar de
la pestaña). El caret se dibuja como un token más (`|`, kind 6) en la línea
activa: no hay que calcular píxeles. El modelo de líneas se muta **por fila**
(`VecModel::set_row_data`) al tipear y al mover el cursor; sólo se reconstruye
entero al abrir, cambiar de pestaña o cambiar el número de líneas.

| Tecla | Qué hace |
|---|---|
| Texto / Tab | Inserta (Tab = 4 espacios). |
| Enter | Parte la línea y **auto-indenta** copiando el espaciado inicial. |
| Backspace / Delete | Borra hacia atrás / adelante, uniendo líneas si hace falta. |
| ← → ↑ ↓ | Mueve el cursor (cruza de línea). |
| `Shift` + flechas/Home/End | Extiende la selección. |
| `Ctrl+A` | Selecciona todo el buffer. |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copiar / cortar / pegar (con selección; sin selección, la línea actual). Usa el portapapeles del sistema (`arboard`). |
| Home / End | Principio / final de la línea. |
| PageUp / PageDown | 10 líneas arriba / abajo. |
| `Ctrl+S` | Guarda (conserva el salto de línea final del original). |
| `Ctrl+Z` / `Ctrl+Y` | Deshace / rehace (pila de 500 por archivo; `Ctrl+Shift+Z` también rehace). |
| `Ctrl+F` | Abre la barra de búsqueda. |
| `Ctrl+H` | Abre la búsqueda con el campo de reemplazo. |
| `F5` | Compila el archivo activo a binario nativo (AOT) y lo ejecuta. |
| `Ctrl+B` | Compila el archivo activo. |
| `F6` | Analiza el archivo activo con el parser de SpASM (diagnóstico). |
| `F8` / `Shift+F8` | Salta al diagnóstico siguiente / anterior. |
| `F12` | Va a la definición del identificador bajo el cursor (esquema del AST). |
| Clic en una fila | Ubica el cursor en la columna más cercana a la x. |
| Arrastrar | Selecciona con el mouse, cruzando líneas. |

**Diagnóstico (lógica real de SpASM).** Al abrir o guardar un `.spasm` (o con
`F6`), NuIde corre el **parser/analizador de SpASM** (`spasm-asm-gen.py`) en un
hilo aparte y muestra el primer error: la línea se marca en rojo en el margen
(barra y número) y el mensaje aparece en la **barra de estado**. Requiere Python
y el frontend del compilador (se descubre junto a `spasmc.py`, en
`F:\SpAMS-Oficial`, o con `NUIDE_SPASM_FRONTEND`).

**Esquema e ir a definición.** De la misma corrida del parser sale el **esquema**
del archivo (funciones y variables del AST): el botón **Esquema**, en la barra de
pestañas, abre la lista y un clic salta a la línea; **`F12`** va a la definición
del identificador bajo el cursor.

**Visores de documentos.** Las imágenes (`.png`, `.jpg`, `.webp`, …) se abren en
una pestaña y se muestran (no se editan), con **zoom** (botones `−` / `+` /
`Ajustar`) y **paneo** arrastrando (o con la rueda). Los `.md` tienen un botón
**Vista / Código** (o `Ctrl+Shift+M`) que alterna entre el código y una **vista
previa** con negrita, itálica, código, links y listas (headings no soportados por
el `StyledText` de Slint: se convierten a negrita). El lenguaje del archivo se
muestra en la barra de estado.

**Buscar y reemplazar.** Con la barra abierta, el teclado edita la búsqueda:
`Enter` / `Shift+Enter` saltan a la coincidencia siguiente / anterior, `Tab`
alterna entre los campos de buscar y reemplazar, y `Esc` cierra. Los campos se
editan con `←`/`→`, `Home`/`End`, `Backspace` y `Delete` (con el caret a la
vista); `↑`/`↓` recorren el **historial** de búsquedas. El botón `Aa` distingue
mayúsculas, `Palabra` busca palabras completas, `.*` interpreta la consulta como
**expresión regular** (`regex-lite`) y `Sel.` busca **sólo dentro de la
selección**. El botón `⇄` muestra u oculta el reemplazo; «Reemplazar» cambia la
coincidencia actual y «Todo» el resto del archivo (ambos con deshacer). El
contador muestra `actual/total`.

**Ayudas de edición.** Al escribir `(`/`[`/`{`/`"`/`'` se inserta el par y el
cursor queda en el medio (y escribir el cierre ya presente sólo avanza sobre él).
`Enter` copia la indentación y **abre un nivel** tras un abridor, dejando el
cierre alineado. `Tab` inserta espacios (o `\t`); el ancho y el modo se cambian
desde la paleta (`Editor: tamaño de tab`, `Editor: insertar espacios`) y se
recuerdan. El **cursor de cada pestaña se guarda** y se recupera al reabrir.

**Resaltados de contexto.** Además del resaltado de sintaxis, el editor pinta
todas las apariciones de la **palabra bajo el cursor**, el **par de llaves**
correspondiente cuando el cursor está sobre una, y las **coincidencias de la
búsqueda** (la actual con un tono más marcado).

La selección se pinta marcando los caracteres como kind 7: el render los envuelve
en un `Rectangle` con fondo `selection`, así no hay que calcular píxeles. El
arrastre va sobre un `TouchArea` que cubre todo el lienzo (no una zona por fila),
con lo que puede cruzar líneas; la rueda del mouse sigue scrolleando la
`Flickable`. Cuando el cursor se mueve, `changed active-line` ajusta la
`content-y` para dejar visible la línea activa (auto-scroll).

Sin pestañas abiertas se muestra una **página de bienvenida**
(`ui/ide_welcome.slint`) con accesos (**Abrir carpeta…**, **Abrir archivo…**,
**Nuevo archivo…**), la lista de **Recientes** (proyectos y archivos, guardados
en la config) y los atajos; con la primera pestaña, el editor pasa a ser real.

### Buscar en el proyecto

`Ctrl+Shift+F` (o el comando *Buscar en el proyecto*) abre un panel que recorre
**todos los archivos del proyecto** y muestra los resultados agrupados por
archivo y línea, con el texto de la línea.

- La búsqueda corre en un **hilo** (`src/search.rs`) y descarta respuestas viejas
  por generación, así escribir no congela la UI. Reutiliza `collect_files`, que
  saltea generados (`target`, `node_modules`, `.git`, …) y carpetas ocultas, y
  saltea binarios y archivos de más de 2 MB.
- `Aa` distingue mayúsculas y `.*` usa **expresiones regulares**.
- `↑`/`↓` navegan, `Tab` pasa al campo de reemplazo y `Enter` (o clic) **abre el
  archivo en la línea** del resultado.
- `⇄` muestra el reemplazo y **Reemplazar todo** lo aplica en todos los archivos
  con coincidencias (con confirmación); recarga las pestañas abiertas afectadas y
  refresca Git.

### La terminal local (real)

`src/pty.rs` + `src/vt.rs` implementan una **terminal de verdad**: una shell
persistente en un pseudo-terminal (**ConPTY** en Windows vía `portable-pty`),
cuyos bytes crudos interpreta el emulador VT de `vt100`. `vt.rs` convierte la
pantalla a tramos de texto con frente/fondo/negrita, y el cursor se dibuja
invirtiendo la celda (sin calcular píxeles).

- La shell por defecto es PowerShell (`pwsh`/`powershell`); se puede forzar con
  `NUIDE_SHELL`. Si no hay ninguna, cae a `cmd.exe`.
- El panel **se redimensiona con la ventana**: la UI le avisa a Rust el tamaño y
  Rust ajusta el PTY (SIGWINCH) y el emulador.
- **Scrollback de 2000 líneas.** La rueda del mouse y `Shift+PageUp`/`PageDown`
  recorren el historial; una barra fina a la derecha indica la posición. Mientras
  se mira hacia atrás, la salida nueva no mueve la vista (la ancla `vt100`).
- **Selección con el mouse y copia.** Se arrastra sobre la pantalla y al soltar
  se copia al portapapeles del sistema; `Ctrl+Shift+C` vuelve a copiar y
  `Ctrl+Shift+V` pega (el texto se le manda al PTY). Cualquier tecla normal
  vuelve al final y descarta la selección.
- Varias sesiones (pestañas) con `+`, cerrar con `×`. Cada una es una shell
  propia. **«Dividir»** parte el panel en **dos vistas** lado a lado (la segunda
  usa otra sesión); clic en una vista le da el foco y el teclado va a esa shell.
  Desde la paleta se **renombra** (`Terminal: renombrar sesión…`) y se
  **reinicia** (`Terminal: reiniciar sesión`, arranca de nuevo en la raíz del
  proyecto).
- **Mouse reporting.** Si el proceso pide el mouse (`vt100` expone el modo), los
  clics, el movimiento y la rueda se **codifican y se le mandan** (SGR, UTF-8 o
  X10) en vez de seleccionar; si no, el mouse queda para selección/copia y
  scrollback. Cada vista (local y agente) respeta su propio modo.
- El teclado se traduce a los bytes del terminal (flechas, Backspace, Delete,
  Home/End, PageUp/Down, Ctrl+letra, Shift+flechas).
- **Grilla exacta**: Slint no admite listas de fuentes, así que la IDE elige una
  familia monoespaciada instalada (JetBrains Mono / Cascadia Mono / Consolas;
  configurable en **Preferencias**) al arrancar, **mide** el ancho real de una
  celda y cada tramo se ubica por su columna (no por el ancho del texto). Así las
  TUI con glifos de caja/bloque (la de `opencode`) quedan alineadas.

Los menús **Ejecutar** y **Compilar** de la barra superior actúan sobre el
archivo activo: **compilan** el `.spasm` a binario nativo (AOT); `spasmc.py`
transpila a C (`--target c`) y el compilador de C detectado produce el `.exe`,
**capturando la salida** en una sesión de terminal dedicada (pestaña `build`). Los
errores y warnings que reporta el toolchain se parsean con su línea y se marcan
en rojo en el editor; **`F8` / `Shift+F8`** saltan al diagnóstico siguiente /
anterior y el mensaje va a la barra de estado.

#### Configuración de ejecución, plantillas y ayuda

- **`F5`** **compila y ejecuta** el **archivo de entrada** configurado (o el
  activo) con los **argumentos** guardados. Desde la paleta: *Ejecutar: elegir
  archivo de entrada…*, *usar el archivo activo*, *argumentos…* y *compilar al
  guardar*. La config se recuerda (`run.entry`, `run.args`,
  `run.build-on-save`).
- **Plantillas** (`src/templates.rs`): *Plantilla: programa completo / hola
  mundo (simple) / archivo vacío…* crean un `.spasm` con el diálogo de
  guardar. *SpASM: nuevo proyecto…* elige una carpeta, crea `src/main.spasm` y la
  fija como proyecto.
- **Ayuda de sintaxis** (`F1` o el comando): la paleta lista las palabras clave y
  builtins de SpASM, filtradas por la **palabra bajo el cursor**; al elegir una,
  se inserta en el cursor.

> **Depurador.** El toolchain de SpASM **no** expone un modo de paso a paso,
> breakpoints ni inspección de registros (`spasm.exe --help` no los ofrece), así
> que un depurador interactivo queda pendiente de agregarlo al intérprete
> (documentado en `docs/PLAN.md`, Fase 7).

> **Flujo de trabajo.** NuIde **compila** el código SpASM a binario nativo (AOT)
> — el intérprete (`spasm.exe`) **no se usa** en la IDE. Con **Ejecutar** (`F5`),
> **Compilar** (`Ctrl+B`) o **Compilar y ejecutar** (`Ctrl+Shift+B`), `spasmc.py`
> transpila a C (`--target c`) y el compilador de C que detecte
> (`gcc`/`clang`/`cc`/`tcc` en el `PATH`, o **MSVC `cl`** cargando
> `vcvars64.bat`) produce el `.exe`. Ambos dialectos (simple y `programa`)
> compilan. Si no hay compilador de C, avisa y deja el `.c`.

#### Dónde busca el toolchain

`src/spasm.rs` no hardcodea rutas. Busca cada herramienta en este orden:

1. Variables de entorno: `NUIDE_SPASM`, `NUIDE_SPASMC`, `NUIDE_PYTHON` (también
   acepta `SPASM_EXE`, `SPASMC`, `PYTHON`), `NUIDE_SPASM_FRONTEND` (el parser,
   `spasm-asm-gen.py`) y `NUIDE_AGENT` para el CLI de OpenCode.
2. Ubicaciones conocidas del ecosistema: `F:\SpASM\spasm.exe`,
   `F:\SpAMS-Oficial\src\shared\spasmc.py` y su frontend
   `...\src\common\spasm-asm-gen.py`.
3. El `PATH`.

#### Nota sobre el Skill PTY

El Skill `pty` de SpASM (`F:\SpAMS-Oficial\skills\pty`) es todavía un esqueleto:
declara `pty_open`/`pty_spawn`/`pty_read`/… pero el intérprete `spasm.exe`
instalado no los implementa. Por eso la terminal de NuIde **no** depende de ese
Skill: implementa su propio PTY nativo (ConPTY) en Rust.

La consola *lineal* de `src/terminal.rs` queda como utilidad heredada (la usan
pruebas de integración del toolchain). El panel del agente ya no la usa.

### El panel derecho (terminal + OpenCode CLI)

El panel derecho es una **terminal real** (shell pwsh/cmd en la raíz del
proyecto), igual que la inferior. `opencode` **no se abre solo**: se lanza
cuando el usuario elige *Lanzar opencode* en la paleta. Si salís de opencode,
la terminal queda en la shell y podés relanzarlo cuando quieras. Su TUI
interactiva se renderiza con el mismo emulador (`vt`), con scrollback,
selección/copia y las mismas teclas. El canal es propio (`agent_tx`/`agent_rx`)
para no mezclar la salida con la terminal inferior.

- Al arrancar se lanza una shell (pwsh/cmd) en la raíz del proyecto, sin
  inyectar nada.
- **Lanzar opencode** (paleta) inyecta el comando de la CLI —entre comillas,
  para que funcione aunque la ruta tenga espacios— en la shell abierta.
- Si salís de opencode, queda en la shell y podés relanzarlo de nuevo desde la
  paleta o tipeando el comando directamente.
- Si la shell se cae (opencode al salir con `Ctrl+C` puede arrastrar la
  consola), el panel **relanza la terminal solo** y vuelve a quedar operativo.
- Al cambiar de proyecto, se reinicia la terminal: la shell nueva arranca en el
  cwd del proyecto (opencode no se relanza).
- El encabezado y el modelo son configurables con `NUIDE_AGENT_VERSION` y
  `NUIDE_AGENT_MODEL`.
- El CLI se descubre como `opencode.cmd` (Windows), `opencode.exe` u `opencode`.
- El **mouse reporting** funciona igual que en la terminal local: si la TUI lo
  pide, los clics/rueda se reenvían; si no, el mouse selecciona/copia.

### El control de código (Git)

Panel **real** de Git por la línea de comandos (sin dependencias): al abrir un
proyecto se lee `git status --porcelain=v1 -b` y se muestran la **rama** y los
archivos con cambios, separados en **CAMBIOS** y **PREPARADOS**.

- Cada fila: estado de dos columnas (`M `, ` M`, `??`, `A `…), la ruta, y un
  botón **+**/**−** para preparar o quitar del índice (`git add`/`git reset`).
  **Clic en la fila abre el diff** del archivo.
- **Preparar todo** (`git add -A`), **Quitar todo** (`git reset`), **Bajar**
  (`git pull`), **Push** y **Commit**. **⟳** relee el estado. El campo de
  mensaje alimenta Commit y **Enmendar** (`--amend`, desde la paleta).
- El encabezado muestra **↑ahead ↓behind** y la **rama es clickeable**: abre un
  menú con las ramas locales (la actual con `●`) y **Nueva rama…**.
- **Visor de diff** (`ui/ide_diff.slint`): vista unificada con colores, y botones
  **Abrir archivo**, **Preparar/Quitar** y **Descartar cambios** (con
  confirmación).
- Si la carpeta no es un repositorio, ofrece **Inicializar repositorio**
  (`git init`).
- Las operaciones corren en un **hilo** (como el linter y el build) para no
  congelar la UI; al volver, se refresca el estado. El panel se relee al abrir
  un proyecto, al guardar y al terminar cada operación.
- La rama también se muestra en la barra de estado.

**Limitación:** no hay *stage* por hunks ni *blame*/*log*; el manejo de
credenciales es el de `git` (si `push`/`pull` piden credenciales, se comporta
como la CLI).

### La paleta de comandos y el buscador de archivos

`Ctrl+Shift+P` abre la **paleta de comandos** y `Ctrl+P` el **buscador de
archivos**; se cierran con `Esc` y se navegan con `↑`/`↓` + `Enter` (o clic).

- La paleta (`ui/ide_palette.slint`) es un overlay oscurecido con un campo de
  filtrado que corre en Rust: se escribe y la lista se recalcula en vivo
  (coincide por etiqueta o por atajo). Incluye ejecutar/compilar/analizar,
  buscar, ir a definición, guardar, abrir carpeta/archivo/nuevo, nueva
  terminal, dividir, `Ver:` cada panel y acciones de Git.
- El buscador junta los archivos del proyecto (`collect_files`, recursivo y con
  topes; saltea `target`/`node_modules`/`.git`/… y carpetas ocultas), filtra por
  ruta y abre el elegido.
- El teclado lo maneja Rust (no un `TextInput`): los tres focos (editor,
  terminal, agente) desvían las teclas a la paleta mientras está abierta, así
  que el atajo es global venga de donde venga el foco.

**Limitación:** el filtrado es por subcadena (sin *fuzzy* ni regex); la lista
muestra hasta 50 archivos.

### Menús y flujo de archivos

**Archivo** y **Editar** tienen su desplegable real (`ui/ide_menu.slint`), con
las filas que arma Rust:

- **Archivo:** Nuevo archivo… (`Ctrl+N`), Abrir archivo… (`Ctrl+O`), Abrir
  carpeta…, Guardar (`Ctrl+S`), Guardar como… (`Ctrl+Shift+S`), Cerrar pestaña
  (`Ctrl+W`), Cerrar todas y Salir.
- **Editar:** Deshacer/Rehacer, Cortar/Copiar/Pegar, Seleccionar todo, Buscar
  (`Ctrl+F`), Reemplazar (`Ctrl+H`) e Ir a línea… (`Ctrl+G`).
- **Guardar como** escribe en la ruta elegida y reapunta la pestaña (idioma,
  miga de pan y diagnósticos incluidos).
- **Cerrar** (pestaña, todas, salir o la ventana) con cambios sin guardar abre un
  diálogo `ui/ide_confirm.slint` (**Guardar / Descartar / Cancelar**).

Ir a línea reusa la paleta en modo numérico. Todo esto también está en la paleta
de comandos (`Ctrl+Shift+P`).

### Preferencias y atajos

`Ctrl+,` (o el comando *Preferencias*) abre `ui/ide_prefs.slint`:

- **Apariencia**: fuente del código y de la interfaz (cicla entre las familias
  instaladas), **tamaño del código** (`−`/`+`) y **acento** (5 colores).
- **Editor**: tamaño de tab (2/4/8) e insertar espacios.
- **Ejecutar**: compilar al guardar.
- **Atajos**: clic en una fila y **presioná la combinación nueva** (`Esc`
  cancela). Hoy son configurables los globales: paleta, buscar archivos, buscar
  en el proyecto y ayuda.

Todo se aplica **en caliente** (se tocan los globales de `IdeTheme`) y se guarda
en la config (`font.*`, `theme.accent`, `keys.*`). El tamaño del código llega al
editor vía `IdeTheme.editor-font-size`.

### Proveedores de OpenCode (configuración del CLI)

La pantalla de configuración (`Ctrl+,`) tiene una sección **Proveedores** para
dar de alta, editar y eliminar los proveedores del CLI de OpenCode sin tocar el
JSON a mano. Edita la config global (`~/.config/opencode/opencode.json(c)`)
tratando `opencode.json` y `opencode.jsonc` como una **unión**: al guardar, todo
queda en `opencode.json` (se crea si falta) y se retira `providers` del `.jsonc`.

- La escritura es **quirúrgica**: sólo se reemplaza el fragmento `providers`, así
  que `$schema`, `model` y el resto del archivo quedan intactos. Los campos se
  parchean *in-place*, conservando claves desconocidas (`env`, `modelID`,
  `limit`, `cost`, …).
- Campos: **ID** (la clave; renombrarlo mueve el proveedor), **Nombre**,
  **Package**, **baseURL**, **apiKey** (oculta) y **Modelos** como **JSON crudo**
  (se valida al guardar; debe ser un objeto).
- No pisa credenciales por entorno: si un proveedor usa `env` y dejás apiKey
  vacía, no se escribe `"apiKey": ""` encima.
- **Eliminar** pide confirmación. `Esc` cierra el formulario.

## Estructura

```
nuide/
  Cargo.toml
  Cargo.lock               lock del proyecto (app independiente)
  build.rs                 compila la lógica SpASM (cc) + slint_build
  tools/regen_spasm.ps1     refresca spasm/generated/*.c con spasmc.py
  spasm/
    *.spasm                lógica de la IDE en SpASM AOT
    generated/*.c          C generado (commiteado; no editar a mano)
    ffi/*_ffi.c            wrappers C con firmas estables
    README.md              reglas del puente Rust ↔ SpASM
  src/
    lib.rs                 pub mod editor + explorer + highlight + pty + spasm + spasm_logic + terminal + ui + vt
    ui.rs                  slint::include_modules!()
    spasm.rs               descubrimiento del toolchain (spasm/spasmc/cc/opencode)
    pty.rs                 terminal real: ConPTY + hilos de lectura/escritura
    vt.rs                  emulador VT: pantalla → tramos con color
    lint.rs                diagnóstico de SpASM (corre el parser real)
    build.rs               ejecuta el toolchain y parsea la salida (build/run)
    git.rs                 control de código por CLI (status, stage, commit, push)
    search.rs              buscar en todo el proyecto (en un hilo)
    templates.rs           plantillas de archivos SpASM
    terminal.rs            consola lineal heredada (sólo pruebas de integración)
    explorer.rs            árbol de archivos plano con profundidad
    highlight.rs           tokenizador/resaltado de SpASM
    config.rs              configuración persistida (parse en SpASM)
    spasm_logic.rs         FFI de la lógica SpASM (wrappers seguros + tests)
    jsonx.rs               JSON con spans: edición quirúrgica de la config
    providers.rs           CRUD de proveedores de OpenCode (config global)
    editor.rs              estado de la IDE (explorador, editor, terminales) + wiring
    main.rs                crea la ventana, engancha y corre
  ui/
    ide_theme.slint        paleta y métricas del IDE
    ide_types.slint        structs compartidos (nodos, tokens, líneas…)
    ide_topbar.slint       barra superior (menú, proyecto, controles)
    ide_statusbar.slint    barra inferior (rama, lenguaje, cursor)
    ide_explorer.slint     árbol de archivos
    ide_editor.slint       pestañas + ruta + área de código
    ide_terminal.slint     terminal local + `TerminalScreen` reutilizable
    ide_opencode.slint     panel de OpenCode CLI (TUI en PTY)
    ide_git.slint          panel de control de código (Git)
    ide_palette.slint      paleta de comandos / buscador de archivos
    ide_search.slint       buscar/reemplazar en el proyecto
    ide_diff.slint         visor de diff de Git
    ide_prefs.slint        preferencias y atajos
    ide_about.slint        diálogo "Acerca de"
    ide_provider.slint     formulario de proveedor de OpenCode
    ide_menu.slint         menú desplegable (Archivo / Editar / Ayuda)
    ide_confirm.slint      diálogos de confirmación / eliminar
    ide_prompt.slint       diálogo de entrada de texto
    ide_viewmenu.slint     desplegable del menú Ver (paneles)
    ide_welcome.slint      bienvenida del editor (accesos + atajos)
    nuide_ui.slint         la ventana: layout, divisores y datos demo
  res/                     nuide.ico + nuide.rc (icono y versión de Windows)
  packaging/               NuIde.iss (Inno Setup) + postinstall.ps1 + make_icon.ps1
  docs/NUIDE_IDEA.md       especificación visual/funcional
  docs/ESTADO.md           parte de estado (hecho, limitaciones, pendientes)
  docs/PLAN.md             plan en fases para llegar a v1.0
  docs/GUIA.md             guía de usuario
```

## Compilar y correr

```sh
cargo run --bin nuide
cargo build --release --bin nuide
```

Para iterar la UI sin compilar:

```sh
slint-viewer ui/nuide_ui.slint
slint-viewer --check ui/nuide_ui.slint
slint-viewer --screenshot ui.png ui/nuide_ui.slint
```

### Empaquetado (Windows)

- El `.exe` de release lleva **icono y metadatos de versión**. `build.rs` compila
  `res/nuide.rc` con `rc.exe` (de los Windows Kits; se puede forzar con `RC`). Si
  no está, avisa y compila igual, sin recursos.
- El binario de release es de **subsistema ventanas** (sin consola): al abrirlo
  desde el Explorador o el menú Inicio **no aparece una terminal vacía**. En
  `debug` se deja la consola; en release, `--version`/`--help` se enganchan a la
  consola del proceso padre (`AttachConsole`) para seguir imprimiendo, pero sólo
  cuando no hay salida asignada: si la salida ya está redirigida (archivo, caño,
  logs de la CI) se respeta y no se pisa con `CONOUT$`.
- El icono se regenera con:
  `powershell -ExecutionPolicy Bypass -File packaging/make_icon.ps1`.
- **Instalador** (`packaging/NuIde.iss`, Inno Setup 6):

  ```sh
  cargo build --release
  iscc packaging\NuIde.iss      # deja releases\NuIde-<version>-setup.exe
  ```

  O todo junto (compila, busca `ISCC.exe` si no está en el `PATH` —la instalación
  por usuario lo deja en `%LOCALAPPDATA%\Programs\Inno Setup 6`— y verifica el
  subsistema del `.exe`):

  ```sh
  powershell -ExecutionPolicy Bypass -File packaging\make_installer.ps1
  ```

  Instala NuIde, crea accesos directos y puede **asociar `.spasm`**, **instalar
  Node.js** (vía `winget`) y **las CLI de IA** (`packaging/postinstall.ps1`; hoy
  `opencode-ai`, ampliable a kilo code / mimo). Las CLI son de Node, por eso Node
  es una dependencia.
- La **CI** (`.gitlab-ci.yml`) corre `fmt`/`clippy -D warnings`/`test` en Linux y
  publica el `.exe` de release como artefacto en Windows.

### Despliegue del sitio

Cada proyecto tiene su **sitio** (host, usuario, contraseña, carpeta remota),
guardado en el config de NuIde **indexado por proyecto** — nunca dentro del
repositorio. Se configura en **Preferencias (`Ctrl+,`) → Despliegue** y se prueba
o sube desde ahí mismo.

- **Transportes**: FTP (21), FTPS (FTP + TLS; curl valida el certificado y acepta
  una CA propia) y SFTP (SSH, 22).
- **Sin crates de red**: las transferencias las hace `curl.exe` (viene con
  Windows, habla `ftp`/`ftps`/`sftp`) o `scp` de OpenSSH, como procesos hijos.
- **Terminal SSH**: abre una sesión `ssh user@host` en una pestaña de terminal
  real (ConPTY).
- **La lógica de qué subir puede ser un script SpASM** del proyecto
  (`deploy.spasm`, que la IDE **compila AOT** y corre). El protocolo es por
  líneas:

  ```text
  UPLOAD css/site.css              # sube a <carpeta-remota>/css/site.css
  UPLOAD build/app.js public/app.js
  SKIP build/grande.wasm pesado
  LOG 3 archivos pendientes
  ```

  Sin script, se sube la *subcarpeta local* (p. ej. `build`) filtrando por
  extensión. El progreso se ve en la sesión `build` de la terminal.

`src/site.rs` (modelo, rutas, argv) y `src/deploy.rs` (hilo, escaneo, protocolo).

### Módulos (Preferencias → Módulos)

Dos extras que se prenden y apagan, **sin dependencias nuevas**:

- **Servidor web local con CORS**: HTTP/1.1 hecho con `std::net`, sirve la
  subcarpeta que elijas del proyecto en `http://localhost:<puerto>/`. Cabeceras
  `Access-Control-Allow-*` configurables (origen, métodos, encabezados),
  preflight `OPTIONS`, `index.html` en directorios y bloqueo de *path traversal*
  (403). Escucha sólo en `127.0.0.1`; cada petición servida se ve en la terminal
  `build`.
- **Visor de SQLite**: panel con las tablas de una base `.db` y sus filas (hasta
  200). No implementa el formato de SQLite: le pregunta a `sqlite3.exe` o a
  Python (módulo `sqlite3`) y le lee TSV, el mismo estilo de contrato por líneas
  del despliegue.

`src/webserver.rs`, `src/sqlite.rs` y `ui/ide_sqlite.slint`.

### Rendimiento

Medido con la app abierta y el panel de OpenCode redibujando (**CPU en reposo /
memoria**):

| | antes | después |
|---|---:|---:|
| CPU de un núcleo (mediana) | ~77 % | ~10 % |
| Memoria (working set) | 113 MB | 40 MB |

Qué lo cambiaba, en orden de impacto:

1. **Renderer**: Slint elegía FemtoVG (OpenGL) por defecto y ése repinta toda la
   escena en cada cambio. `pick_renderer()` (`src/main.rs`) usa el renderer
   **software** de Slint (partial rendering) si no se seteó `SLINT_BACKEND`.
   Para probar la vía GPU: `$env:SLINT_BACKEND = "winit-femtovg"`.
2. **Recreación de elementos**: las filas de terminal se cambiaban con
   `remove`+`insert` (`row_removed`/`row_added` → Slint destruía y creaba el
   elemento y sus `Text`). Ahora hay un modelo propio, `TermRows`, que muta la
   fila y avisa con `row_changed`.
3. **Refresco acotado**: la salida del PTY refresca **sólo** los paneles de
   terminal (`sync_terms`), no los ~20 modelos de la ventana; `Screen` calcula
   una huella por fila y `changed_rows()` entrega sólo las que cambiaron; el
   medidor de CPU/RAM escribe sus dos propiedades sin provocar un vuelcado; y un
   panel oculto no se refresca.

Detalle en [`docs/ESTADO.md`](docs/ESTADO.md) (§3.25), con lo que queda pendiente.

## Interacción

- **Divisores arrastrables**: un `TouchArea` estacionario detrás de los paneles
  detecta la zona pulsada (huecos de 6 px) y reparte el espacio con
  `left-frac` / `right-frac` / `bottom-frac`. Cambiar la terminal inferior solo
  afecta a la columna central.
- **Layout modular**: cada panel —**Explorador**, **OpenCode CLI** y
  **Terminal**— tiene un **dock** (izquierda, derecha, abajo u oculto). Los que
  comparten lado **se apilan** (hasta 3 por lado). Se controla desde dos lugares:
  los **botones de layout** de la barra superior (prenden/apagan cada región:
  izquierda, derecha, inferior) y el menú **Ver**, donde cada fila muestra dónde
  está el panel y un clic lo cicla (izquierda → derecha → abajo → oculto). El
  editor ocupa el espacio libre y el dock elegido se recuerda entre ejecuciones.
- **Arrastrar y soltar paneles**: cada cabecera tiene un **asa** (las tres rayitas
  a la izquierda). Agarrarla y soltarla sobre una banda la mueve de dock: se
  resaltan **Izquierda / Derecha / Abajo** y el centro significa **ocultar**. Una
  etiqueta sigue al cursor con el destino. Se persiste igual que el menú **Ver**
  (`panel.<i>.dock`), y si la región de destino estaba apagada se enciende.
  Implementación: `ui/ide_dock.slint` (global `DockDrag` + `DockHandle`) y
  `Ide::dock_drop` en `src/editor.rs`.
- **Opacidad de los paneles**: **Preferencias → Opacidad de los paneles** cicla
  100 % → 92 % → 85 % → 75 % → 65 %. Compone las superficies contra el fondo de
  la ventana. (Transparencia *al escritorio* no está disponible: el backend winit
  de Slint crea la ventana principal con `with_transparent(false)`.)
- **Cada área hace su propio scroll**: el explorador y el editor con `Flickable`;
  las dos terminales con el scrollback del emulador `vt` (rueda y barra).
- **Selección por índice**: la UI guarda índices (`explorer-selected`,
  `active-tab`, `active-session`) en vez de reconstruir los modelos.
- **Explorador y editor reales**: clic en carpeta abre/cierra; clic en archivo lo
  abre en una pestaña con resaltado SpASM. La raíz del árbol se elige con clic en
  el nombre del proyecto (diálogo nativo) y se recuerda entre arranques.
- **Foco de teclado**: `IdeTerminal`, `IdeEditor` e `IdeOpenCode` heredan de
  `FocusScope` y tienen `focus-on-click`. La ventana hace `forward-focus` a la
  terminal local, así que ésta recibe el teclado apenas se abre; clic en el
  editor le pasa el foco (para tipear código), y clic en cualquiera de las dos
  terminales lo devuelve.

## Próximos pasos

Ordenados por impacto para usar el IDE día a día; el **plan en fases** completo
(con tamaño, archivos y criterio de terminado de cada una) está en
[`docs/PLAN.md`](docs/PLAN.md).

1. **Mouse reporting en las terminales.** Reenviar el mouse a los procesos que lo
   pidan (la TUI de OpenCode, `vim`…) sin perder la selección/copia.
2. **Empaquetado.** `.exe` release con icono y versión, e instalador, como las
   otras apps del ecosistema.
3. **Persistencia ampliada y búsqueda avanzada.** Recordar también las sesiones
   de terminal, y sumar *fuzzy*/regex y acotar la búsqueda a la selección.

### Estado al 2026-10-08

Compilado y publicado: `github.com/nutoysaudio-code/NuIde` (`main`). 159 tests OK;
clon limpio genera `nuide.exe`. Las dos terminales son PTY reales (shell local y
TUI de `opencode`) con **mouse reporting**, el proyecto se elige con diálogo
nativo y se recuerda, el editor tiene buscar/reemplazar (con **regex**, **en
selección** e historial), autocierre/autoindentación, resaltados, **resaltado
por lenguaje y visores** (imagen y vista previa de Markdown), **diagnóstico con
el parser de SpASM**, **control de código (Git)** con **diff, ramas, pull y ahead/behind**,
**paleta de comandos / buscador de archivos**, **buscar en todo el proyecto**, **menús Archivo/Editar**
con guardar como y cierre confirmado, **explorador con operaciones de archivos**
y un **CRUD de proveedores de OpenCode**; los paneles se acoplan/apilan por lado.
Falta local: los ítems de arriba.
