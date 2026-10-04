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
  auto-indentación y resaltado de SpASM. **Buscar y reemplazar** (`Ctrl+F` /
  `Ctrl+H`) con contador, navegación, mayúsculas, **palabra completa** y edición
  del campo con cursor. Resaltados de contexto: palabra bajo el cursor y par de
  llaves. `F5` ejecutar / `Ctrl+B` compilar. Menús Ejecutar/Compilar sobre el
  archivo activo.
- **Terminal local**: shell persistente en ConPTY, varias sesiones, resize,
  scrollback de 2000 líneas, selección y copia con el mouse.
- **Panel del agente**: `opencode` corriendo en un PTY (su TUI real), con los
  mismos gestos que la terminal local.
- **Proyecto**: se elige con el diálogo nativo (clic en el nombre en la barra
  superior) y se recuerda entre arranques.
- **Layout modular**: el menú **Ver** muestra u oculta el explorador, el panel de
  OpenCode y la terminal inferior; el editor ocupa el espacio liberado.
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

- La ventana calcula su geometría según qué paneles están visibles
  (`show-left`, `show-right`, `show-bottom`): el editor se ensancha/alarga y los
  divisores de los paneles ocultos dejan de responder.
- El ítem **Ver** de la barra superior abre un desplegable (`ui/ide_viewmenu.slint`)
  con **Explorador**, **OpenCode CLI** y **Terminal**; cada fila muestra u oculta
  su panel. El menú se ancla bajo el ítem `Ver` (la barra publica su x) y se
  cierra al hacer clic afuera.
- El estado de visibilidad vive en `Ide` y lo vuelca `sync`.

---

## 4. Limitaciones conocidas

- **Sin *mouse reporting*.** El mouse se usa para seleccionar/copiar, así que no
  se reenvía a los procesos que lo piden (la TUI de OpenCode, `vim`, etc.).
- **Persistencia parcial.** Sólo se recuerda la carpeta del proyecto. El tamaño y
  la posición de la ventana, las pestañas abiertas, las sesiones de terminal y la
  visibilidad de los paneles se pierden al cerrar.
- **Sólo dos columnas laterales.** El layout soporta mostrar/ocultar el
  explorador y OpenCode, pero no apilar varias columnas por lado (agregar
  paneles nuevos requiere tocar el layout).
- **La shell local no cambia de carpeta** al cambiar de proyecto (sí lo hacen las
  sesiones nuevas y el agente, que se relanza).
- **Git decorativo.** La barra inferior muestra `main` fijo; no hay estado real ni
  acciones.
- **Sin atajos globales.** `F5`/`Ctrl+B` funcionan con el foco en el editor, pero
  no son globales, y no hay paleta de comandos ni búsqueda de archivos. La
  búsqueda del editor no soporta expresiones regulares ni acotar a la selección.
- **`terminal.rs` heredado.** La consola lineal ya no la usa la aplicación; sólo
  la ejercitan pruebas de integración del toolchain. Candidata a eliminar o
  migrar sus pruebas al PTY.
- **Empaquetado básico.** No hay icono/versión en el `.exe` ni instalador.
- **Alternar proyectos relanza el agente** y pierde su sesión en curso (a
  propósito: el agente trabaja sobre la raíz del proyecto).

---

## 5. Pendientes (hoja de ruta)

Ordenados por impacto para el uso diario.

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

**Objetivo:** `F5` ejecutar / `Ctrl+B` compilar (ya están, con foco en el editor),
paleta de comandos y búsqueda de archivos. En el editor ya están buscar/reemplazar
(`Ctrl+F`/`Ctrl+H`) con mayúsculas y palabra completa; faltan **regex**, acotar a
la selección y hacer los atajos globales.

**Enfoque:** callbacks de teclado globales en `NuIdeWindow`; un overlay para la
paleta y el buscador de archivos; en la búsqueda, sumar banderas.

**Archivos:** `ui/*.slint`, `src/editor.rs`, `src/highlight.rs`.

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
- `cargo test`: **50 pruebas OK** (unitarias; las de PTY lanzan `cmd`/`opencode`
  reales y se saltean si no están).
- `cargo fmt --check` limpio.
- `slint-viewer --check ui/nuide_ui.slint` OK.
- Arranque real del `.exe` sin panics.

### Cómo probar a mano

```sh
cargo run --bin nuide
# UI sin backend (datos demo):
slint-viewer ui/nuide_ui.slint
```

Para el panel del agente hace falta `opencode` en el `PATH` (o `NUIDE_AGENT`).
Para Ejecutar/Compilar, el toolchain de SpASM (ver §"Dónde busca el toolchain"
del README).
