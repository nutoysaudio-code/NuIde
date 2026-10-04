# NuIde — SpASM Studio

IDE de escritorio para **SpASM**: explorador de archivos, editor de código con
resaltado, terminal local y una terminal dedicada a **OpenCode CLI**.

NuIde es un **proyecto independiente**: una aplicación de escritorio en
**Rust + Slint** que no depende del workspace de audio ni de ningún crate
compartido de plugins. Sus únicas dependencias son `slint` (UI) y `arboard`
(portapapeles). Tiene su propio `[workspace]` para compilarse solo.

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
| 5 | terminal local + panel de OpenCode CLI (procesos reales) | ✅ |
| 6 | integración con el toolchain `spasm`/`spasmc` (build/run) | ✅ |

### El explorador (real)

`src/explorer.rs` arma un árbol **plano con profundidad**, que es lo que dibuja
`IdeExplorer`, y guarda el `path` real de cada fila. La raíz sigue a la carpeta
de la sesión activa de la terminal: un `cd` mueve el árbol. Hay topes de 500
entradas por carpeta y 4000 nodos para que un `target/` o un `node_modules/` no
cuelguen la UI.

- Clic en una carpeta: se abre o se cierra.
- Clic en un archivo: se abre en una pestaña, se resalta y queda como archivo
  activo para los menús Ejecutar/Compilar.
- Las extensiones de configuración (`.toml`, `.json`, `.yaml`, …) llevan el
  icono de engranaje.

### El editor (edición + resaltado)

`src/highlight.rs` es un tokenizador de SpASM por línea que colorea comentarios
(`;`, `//`), strings, números (`0x…`, floats), palabras clave/builtins y
etiquetas (`loop:`, `.next_char:`). Los tabs se expanden a cuatro espacios y la
concatenación de tokens reproduce la línea exacta, que es lo que necesita el
render monoespaciado.

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
| Clic en una fila | Ubica el cursor en la columna más cercana a la x. |
| Arrastrar | Selecciona con el mouse, cruzando líneas. |

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

`src/terminal.rs` implementa una **consola lineal**, no un emulador VT100: cada
proceso que se lanza escribe líneas en stdout/stderr y esas líneas se apilan en
el historial de su sesión. La salida se limpia de secuencias ANSI y se etiqueta
por color (normal / error / comando / aviso).

Escribí en el panel `TERMINAL` y apretá Enter. Comandos propios:

| Comando | Qué hace |
|---|---|
| `help` | Lista los comandos propios. |
| `cd <dir>` | Cambia la carpeta de la sesión y re-enraiza el explorador (`cd` solo va al home). |
| `clear` | Limpia el scrollback de la sesión. |
| `open <archivo>` | Abre el archivo en una pestaña del editor y lo marca como activo. |
| `run [archivo]` | Ejecuta un `.spasm` con el intérprete `spasm.exe`. |
| `build [archivo]` | Transpila a C con `spasmc.py --target c --out build/<x>.c`. Si hay `cc`/`gcc` en el `PATH`, además construye el binario (`--build`). |
| *(otra línea)* | Se delega al shell del sistema (`cmd /C` en Windows). |

Cualquier archivo con extensión distinta se pasa al shell tal cual: `dir`,
`git status`, etc. funcionan.

Los menús **Ejecutar** y **Compilar** de la barra superior actúan sobre el
archivo activo (el que marcó `open`, o el último que se ejecutó).

> **Dialectos.** SpASM tiene dos. El intérprete `spasm.exe` corre el *dialecto de
> intérprete* (top-level, `print ...; end;`). El compilador AOT trabaja con el
> *dialecto compilado* (`programa "..."` + `main { ... }`). «Ejecutar» usa el
> intérprete, así que detecta el encabezado `programa` y avisa en vez de fallar
> con un error de sintaxis; esos archivos van por «Compilar».

#### Dónde busca el toolchain

`src/spasm.rs` no hardcodea rutas. Busca cada herramienta en este orden:

1. Variables de entorno: `NUIDE_SPASM`, `NUIDE_SPASMC`, `NUIDE_PYTHON` (también
   acepta `SPASM_EXE`, `SPASMC`, `PYTHON`) y `NUIDE_AGENT` para el CLI de OpenCode.
2. Ubicaciones conocidas del ecosistema: `F:\SpASM\spasm.exe` y
   `F:\SpAMS-Oficial\src\shared\spasmc.py`.
3. El `PATH`.

Si falta alguna, la terminal lo avisa en el banner y lo propio de SpASM queda
deshabilitado; el resto de la consola sigue funcionando.

#### Nota sobre el Skill PTY

El Skill `pty` de SpASM (`F:\SpAMS-Oficial\skills\pty`) es todavía un esqueleto:
declara `pty_open`/`pty_spawn`/`pty_read`/… pero el intérprete `spasm.exe`
instalado no los implementa —`shell_hello.spasm` termina en
`Runtime error: internal runtime failure`—. Por eso la terminal **no** depende de
ese Skill: la consola es nativa. Cuando el runtime de SpASM exponga las
primitivas PTY, el camino es reemplazar el modelo lineal por un PTY con parser
ANSI.

### El panel del agente (OpenCode CLI, real)

El panel derecho es una consola dedicada al CLI de **OpenCode**, con su propio
canal (`agent_tx`/`agent_rx`) para no mezclar sesiones con la terminal local.
Escribí un prompt y Enter: se ejecuta `opencode run <mensaje>` en la raíz del
proyecto y la salida se vuelca ahí.

- `$ <comando>` corre ese comando en el shell del sistema (en la raíz del
  proyecto) en vez de mandarlo al agente.
- Al arrancar se ejecuta `opencode --version` para mostrar la versión real; el
  encabezado dice «Conectado» si se encontró el CLI.
- El encabezado y el modelo son configurables con `NUIDE_AGENT_VERSION` y
  `NUIDE_AGENT_MODEL`.
- El CLI se descubre como `opencode.cmd` (Windows), `opencode.exe` u `opencode`.

**Limitación:** cada envío es un proceso `opencode run` nuevo (modo no
interactivo). No es una TUI persistente; para eso haría falta el PTY.

## Estructura

```
nuide/
  Cargo.toml
  Cargo.lock               lock del proyecto (app independiente)
  build.rs                 slint_build::compile("ui/nuide_ui.slint")
  src/
    lib.rs                 pub mod editor + explorer + highlight + spasm + terminal + ui
    ui.rs                  slint::include_modules!()
    spasm.rs               descubrimiento del toolchain (spasm/spasmc/cc/opencode)
    terminal.rs            consola lineal: sesiones, procesos, ANSI, tokenizado
    explorer.rs            árbol de archivos plano con profundidad
    highlight.rs           tokenizador/resaltado de SpASM
    editor.rs              estado de la IDE (explorador, editor, terminales) + wiring
    main.rs                crea la ventana, engancha y corre
  ui/
    ide_theme.slint        paleta y métricas del IDE
    ide_types.slint        structs compartidos (nodos, tokens, líneas…)
    ide_topbar.slint       barra superior (menú, proyecto, controles)
    ide_statusbar.slint    barra inferior (rama, lenguaje, cursor)
    ide_explorer.slint     árbol de archivos
    ide_editor.slint       pestañas + ruta + área de código
    ide_terminal.slint     terminal local + `TerminalBody` reutilizable
    ide_opencode.slint     panel de OpenCode CLI
    nuide_ui.slint         la ventana: layout, divisores y datos demo
  docs/NUIDE_IDEA.md       especificación visual/funcional
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
- **Cada área hace su propio scroll** (`Flickable`): explorador, código,
  terminal local y OpenCode.
- **Selección por índice**: la UI guarda índices (`explorer-selected`,
  `active-tab`, `active-session`) en vez de reconstruir los modelos.
- **Explorador y editor reales**: clic en carpeta abre/cierra; clic en archivo lo
  abre en una pestaña con resaltado SpASM. La raíz del árbol sigue a la carpeta
  de la sesión activa de la terminal (`cd` la mueve).
- **Foco de teclado**: `IdeTerminal`, `IdeEditor` e `IdeOpenCode` heredan de
  `FocusScope` y tienen `focus-on-click`. La ventana hace `forward-focus` a la
  terminal local, así que ésta recibe el teclado apenas se abre; clic en el
  editor le pasa el foco (para tipear código), y clic en cualquiera de las dos
  consolas lo devuelve.
