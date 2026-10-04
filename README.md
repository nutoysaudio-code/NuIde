# NuIde — SpASM Studio

IDE de escritorio para **SpASM**: explorador de archivos, editor de código con
resaltado, terminal local y una terminal dedicada a **OpenCode CLI**.

NuIde es un **proyecto independiente**: una aplicación de escritorio en
**Rust + Slint** que no depende del workspace de audio ni de ningún crate
compartido de plugins. Sus dependencias son `slint` (UI), `arboard`
(portapapeles), `portable-pty` + `vt100` (terminales reales) y `rfd` (diálogo
nativo de carpetas). Tiene su propio `[workspace]` para compilarse solo.

La paleta es oscura y propia (instrumento electrónico sobrio): vive en
`ui/ide_theme.slint`. La especificación visual está en
[`docs/NUIDE_IDEA.md`](docs/NUIDE_IDEA.md) y la referencia es
`SpASM Studio con terminales integrados.png`.

```sh
cargo run --bin nuide          # o `cargo build --release`
```

---

## Estado

La terminal, el explorador, el editor (edición con resaltado y guardado) y el
panel de **OpenCode CLI** ya son **reales**. La interfaz dejó de tener datos
ilustrativos salvo la pantalla de bienvenida del editor (el código demo que se ve
hasta abrir el primer archivo).

| Fase | Contenido | Estado |
|---|---|---|
| 1 | andamiaje: crate + ventana + tema + layout de 3 columnas | ✅ |
| 2 | divisores arrastrables + scroll por área | ✅ |
| 3 | explorador: abrir/cerrar carpetas y abrir archivos reales | ✅ |
| 4 | editor: edición, guardado y resaltado real de SpASM | ✅ |
| 5 | terminal real (PTY/ConPTY) + panel de OpenCode CLI | ✅ |
| 6 | integración con el toolchain `spasm`/`spasmc` (build/run) | ✅ |
| 7 | terminal: scrollback + selección y copia con el mouse | ✅ |
| 8 | panel del agente: `opencode` en un PTY (TUI real) | ✅ |
| 9 | elegir proyecto (diálogo nativo) + recordarlo | ✅ |
| 10 | editor: buscar/reemplazar + resaltados de contexto | ✅ |
| 11 | layout modular: controles en la barra + menú Ver | ✅ |
| 12 | dock por panel + apilado por lado (hasta 3) | ✅ |
| 13 | diagnóstico real: parser de SpASM en el editor | ✅ |
| 14 | terminales: grilla exacta (ancho de celda medido) | ✅ |
| 15 | editor: resaltado por lenguaje (SpASM, Slint, Python, C, …) | ✅ |
| 16 | visores: imágenes + vista previa de Markdown | ✅ |
| 17 | build/run desde la IDE con errores navegables (`F8`) | ✅ |

> El parte de estado detallado —qué se cambió en cada iteración, limitaciones y
> pendientes con su enfoque— está en [`docs/ESTADO.md`](docs/ESTADO.md).

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

La **raíz** es la carpeta del proyecto: la última elegida o la carpeta de
lanzamiento. Se cambia con **clic en el nombre del proyecto** en la barra
superior, que abre el diálogo nativo de carpetas (`rfd`). Al elegir otra, el
explorador se reconstruye, el agente se relanza en la carpeta nueva y la
elección se recuerda en `%APPDATA%\NuIde\config` para el próximo arranque.

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
| `F5` | Ejecuta el archivo activo con el intérprete de SpASM. |
| `Ctrl+B` | Compila el archivo activo. |
| `F6` | Analiza el archivo activo con el parser de SpASM (diagnóstico). |
| `F8` / `Shift+F8` | Salta al diagnóstico siguiente / anterior. |
| Clic en una fila | Ubica el cursor en la columna más cercana a la x. |
| Arrastrar | Selecciona con el mouse, cruzando líneas. |

**Diagnóstico (lógica real de SpASM).** Al abrir o guardar un `.spasm` (o con
`F6`), NuIde corre el **parser/analizador de SpASM** (`spasm-asm-gen.py`) en un
hilo aparte y muestra el primer error: la línea se marca en rojo en el margen
(barra y número) y el mensaje aparece en la **barra de estado**. Requiere Python
y el frontend del compilador (se descubre junto a `spasmc.py`, en
`F:\SpAMS-Oficial`, o con `NUIDE_SPASM_FRONTEND`).

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
vista). El botón `Aa` distingue mayúsculas, `Palabra` busca palabras completas y
`⇄` muestra u oculta el reemplazo. «Reemplazar» cambia la coincidencia actual y
«Todo» el resto del archivo (ambos con deshacer). El contador muestra
`actual/total`.

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

Sin pestañas abiertas (y sin haber abierto nunca) se mantiene el contenido demo
del `.slint` como pantalla de bienvenida; con la primera pestaña, el editor pasa
a ser real.

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
  propia. «Dividir» abre otra sesión.
- El teclado se traduce a los bytes del terminal (flechas, Backspace, Delete,
  Home/End, PageUp/Down, Ctrl+letra, Shift+flechas).
- **Grilla exacta**: Slint no admite listas de fuentes, así que la IDE elige una
  familia monoespaciada instalada (JetBrains Mono / Cascadia Mono / Consolas) al
  arrancar, **mide** el ancho real de una celda y cada tramo se ubica por su
  columna (no por el ancho del texto). Así las TUI con glifos de caja/bloque (la
  de `opencode`) quedan alineadas.

Los menús **Ejecutar** y **Compilar** de la barra superior actúan sobre el
archivo activo: corren el intérprete (`spasm.exe`) o el compilador (`spasmc.py`)
**capturando la salida** en una sesión de terminal dedicada (pestaña `build`). Los
errores y warnings que reporta el toolchain se parsean con su línea y se marcan
en rojo en el editor; **`F8` / `Shift+F8`** saltan al diagnóstico siguiente /
anterior y el mensaje va a la barra de estado.

> **Dialectos.** SpASM tiene dos: el intérprete `spasm.exe` corre el *dialecto de
> intérprete* (`print ...; end;`) y el compilador AOT el *dialecto compilado*
> (`programa "..."` + `main { ... }`). «Ejecutar» lanza el intérprete y «Compilar»
> lanza `spasmc.py` (con `--build` sólo si hay un compilador de C en el `PATH`).

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

### El panel del agente (OpenCode CLI, TUI real)

El panel derecho corre **`opencode` en un PTY**, igual que la terminal local:
su TUI interactiva se renderiza con el mismo emulador (`vt`), con scrollback,
selección/copia y las mismas teclas. El canal es propio (`agent_tx`/`agent_rx`)
para no mezclar la salida con la terminal inferior.

- Al arrancar se lanza `opencode` (sin argumentos) en la raíz del proyecto; si
  el CLI no está, la pantalla muestra el motivo.
- El encabezado dice «Conectado» mientras el proceso vive; al salir pasa a
  «Desconectado».
- El encabezado y el modelo son configurables con `NUIDE_AGENT_VERSION` y
  `NUIDE_AGENT_MODEL`.
- El CLI se descubre como `opencode.cmd` (Windows), `opencode.exe` u `opencode`.

**Limitación:** el mouse se usa para seleccionar texto, así que no se reenvía a
la TUI (no hay *mouse reporting*). El teclado sí llega completo.

## Estructura

```
nuide/
  Cargo.toml
  Cargo.lock               lock del proyecto (app independiente)
  build.rs                 slint_build::compile("ui/nuide_ui.slint")
  src/
    lib.rs                 pub mod editor + explorer + highlight + pty + spasm + terminal + ui + vt
    ui.rs                  slint::include_modules!()
    spasm.rs               descubrimiento del toolchain (spasm/spasmc/cc/opencode)
    pty.rs                 terminal real: ConPTY + hilos de lectura/escritura
    vt.rs                  emulador VT: pantalla → tramos con color
    lint.rs                diagnóstico de SpASM (corre el parser real)
    build.rs               ejecuta el toolchain y parsea la salida (build/run)
    terminal.rs            consola lineal heredada (sólo pruebas de integración)
    explorer.rs            árbol de archivos plano con profundidad
    highlight.rs           tokenizador/resaltado de SpASM
    config.rs              configuración persistida (carpeta del proyecto)
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
    ide_viewmenu.slint     desplegable del menú Ver (paneles)
    nuide_ui.slint         la ventana: layout, divisores y datos demo
  docs/NUIDE_IDEA.md       especificación visual/funcional
  docs/ESTADO.md           parte de estado (hecho, limitaciones, pendientes)
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

Ordenados por impacto para usar el IDE día a día. (Nada de esto está hecho; es
la hoja de ruta acordada. El enfoque de cada uno está en
[`docs/ESTADO.md`](docs/ESTADO.md#5-pendientes-hoja-de-ruta).)

1. **Persistencia de estado.** Guardar entre ejecuciones algo más que el
   proyecto: tamaño/posición de la ventana, pestañas abiertas, sesiones y qué
   paneles están visibles.
2. **Git.** Barra de estado con rama y estado reales + acciones básicas (stage,
   commit, push) desde la IDE.
3. **Atajos y búsqueda.** `F5` ejecutar / `Ctrl+B` compilar, paleta de comandos y
   búsqueda de archivos; buscar/reemplazar dentro del editor.
4. **Mouse reporting en las terminales.** Reenviar el mouse a los procesos que lo
   pidan (la TUI de OpenCode, `vim`…) sin perder la selección/copia.
5. **Empaquetado.** `.exe` release con icono y versión, e instalador, como las
   otras apps del ecosistema.

### Estado al 2026-10-04

Compilado y publicado: `gitlab.com/nutoys-group/nuide` (`main`). 62 tests OK;
clon limpio genera `nuide.exe`. Las dos terminales son PTY reales (shell local y
TUI de `opencode`), el proyecto se elige con diálogo nativo y se recuerda, el
editor tiene buscar/reemplazar, resaltados, **resaltado por lenguaje y visores**
(imagen y vista previa de Markdown), **diagnóstico con el parser de SpASM**, y
los paneles se acoplan/apilan por lado. Falta local: los ítems de arriba.
