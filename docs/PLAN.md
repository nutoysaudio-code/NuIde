# Plan en fases para completar NuIde

Documento de trabajo. El parte de estado (qué está hecho, limitaciones y
pendientes) vive en [`ESTADO.md`](ESTADO.md); acá está **cómo se termina**.

## Objetivo: qué significa "terminado" (v1.0)

NuIde v1.0 es un IDE de escritorio **usable a diario** para proyectos SpASM,
con estas garantías:

1. **Editar y guardar sin sorpresas**: operaciones de archivo completas
   (crear, renombrar, borrar, guardar como), cierre seguro con cambios sin
   guardar y persistencia de cursor/scroll por archivo.
2. **Encontrar cosas rápido**: buscar/reemplazar con regex y en la selección,
   buscar en todo el proyecto y paleta de comandos (✔ hecha).
3. **Ejecutar y depurar SpASM**: compilar/ejecutar con configuración, errores
   navegables (✔) y un depurador con breakpoints, paso a paso y estado.
4. **Trabajo con Git**: estado real (✔), *diff*, ramas y *pull/push*.
5. **Terminal completa**: PTY real (✔), mouse reporting, sesiones administrables
   y persistentes.
6. **Instalable**: `.exe` release con icono/versión e instalador, publicado por
   CI, como el resto del ecosistema.
7. **Documentado y probado**: guía de usuario, tests y CI en verde.

Lo que ya está hecho y no se repite: layout y docks, editor con resaltado por
lenguaje y visores, terminales PTY (local + agente NuCoder), build/run,
diagnóstico con el parser, esquema e ir a definición, Git (panel), bienvenida,
paleta de comandos y buscador de archivos.

### Cómo se mide cada tarea

- **Tamaño**: `S` (horas), `M` (1–2 días), `L` (varios días).
- **Prioridad**: `must` (sin esto no hay v1.0) / `nice` (mejora).
- Cada fase se cierra con: `cargo fmt --check`, `cargo check` sin warnings,
  `cargo test` en verde, `slint-viewer --check`, y commit + push con docs.

---

## Fase 0 — Base de calidad y CI  ·  S/M  ·  must

**Objetivo:** que cada cambio futuro esté protegido por tests y builds
automáticos antes de sumar features grandes.

- `S` Perfil `release`: `opt-level`, `lto`, `strip` y `panic`.
- `S` `cargo clippy` limpio (o con `-D warnings` en lo nuevo).
- `M` `.gitlab-ci.yml`: etapas `fmt`, `clippy`, `test`, `build` (artefacto
  `nuide.exe`).
- `S` Smoke test del binario (arranca y no paniquea) en el runner.

**Archivos:** `Cargo.toml`, `.gitlab-ci.yml` (nuevo), `README.md`.
**Terminado:** el pipeline corre en cada push y publica el `.exe`.

---

## Fase 1 — Menús y flujo de archivos  ·  M  ·  must  ·  ✅ hecha (3.16)

**Objetivo:** que los menús **Archivo** y **Editar** hagan lo que se espera
(hoy no hacen nada) y que no se pierdan cambios.

- `M` Desplegables de menú reutilizables (un `IdeMenu` con ítems) para Archivo y
  Editar; hoy solo existe el menú Ver.
- **Archivo:** Nuevo, Abrir archivo…, Abrir carpeta…, Guardar, Guardar como…,
  Cerrar pestaña, Cerrar todas, Salir.
- `S` **Guardar como** (diálogo `rfd::save_file`) y **Cerrar pestaña** con
  confirmación si hay cambios sin guardar.
- **Editar:** Deshacer, Rehacer, Cortar, Copiar, Pegar, Seleccionar todo,
  Buscar/Reemplazar, Ir a línea.
- `S` Diálogo de confirmación "¿guardar cambios?" al cerrar la ventana con
  pestañas *dirty*.
- `S` Sumar estos comandos a la paleta.

**Archivos:** `ui/ide_menu.slint` (nuevo), `ui/ide_topbar.slint`,
`ui/nuide_ui.slint`, `src/editor.rs`.
**Terminado:** todo lo anterior funciona desde el menú y desde la paleta.

---

## Fase 2 — Explorador: operaciones de archivos  ·  M/L  ·  must  ·  ✅ hecha (3.17)

**Objetivo:** administrar archivos sin salir de la IDE.

- `M` Menú contextual por fila: Nuevo archivo, Nueva carpeta, Renombrar,
  Eliminar, Copiar ruta, Revelar en el explorador del SO.
- `M` Crear/renombrar con diálogo (o edición inline) y validación de nombre.
- `S` Eliminar a la **papelera** cuando se pueda (o confirmación fuerte si no).
- `S` Refrescar y auto-refresh (al crear/renombrar/borrar y al guardar).
- `S` Mostrar/ocultar archivos generados (`target`, `node_modules`, puntos).
- `nice` Mover/copiar arrastrando entre carpetas.

**Archivos:** `ui/ide_explorer.slint`, `src/explorer.rs`, `src/editor.rs`.
**Terminado:** crear/renombrar/borrar y abrir desde el árbol con refresco
correcto y sin perder estado del editor.

---

## Fase 3 — Editor avanzado  ·  M/L  ·  must + nice  ·  ✅ hecha (3.18)

**Objetivo:** cerrar las carencias del editor frente a VS Code en lo diario.
(Hecho: regex, en selección, historial, autocierre/autoindentación, tab
configurable y persistencia de cursor. Pendiente de esta fase: *word wrap*,
plegado, multi-cursor y *render* virtualizado.)

- `M` Búsqueda con **regex**, **acotada a la selección** e **historial**.
- `S` **Ir a línea** (`Ctrl+G`) y "ir a símbolo" desde el esquema.
- `S` **Autoindentación** y **auto-cierre** de paréntesis/llaves/comillas.
- `S` Tamaño de tab e inserción de espacios **configurable**.
- `M` **Persistencia por archivo**: cursor, scroll y estado de la búsqueda
  (hoy solo se recuerdan las pestañas abiertas).
- `nice` *Word wrap*, *folding* (plegado) y multi-cursor.
- `M` **Render virtualizado** de líneas (hoy se materializa el modelo completo
  hasta 20k líneas): necesario para archivos grandes.

**Archivos:** `src/editor.rs`, `src/highlight.rs`, `ui/ide_editor.slint`.
**Terminado:** buscar/ir con regex y selección, y reabrir un archivo que
recupera cursor y scroll.

---

## Fase 4 — Buscar en el proyecto  ·  M  ·  must  ·  ✅ hecha (3.20)

**Objetivo:** encontrar texto en todos los archivos.

- `M` `Ctrl+Shift+F`: panel de resultados agrupado por archivo con contexto.
- `S` Clic en un resultado → abre el archivo en la línea.
- `M` Reemplazar en el proyecto (con confirmación).
- `S` Respetar `.gitignore` y excluir generados (reusar `collect_files`).

**Archivos:** `src/search.rs` (nuevo), `ui/ide_search.slint` (nuevo),
`src/editor.rs`.
**Terminado:** buscar y reemplazar en todo el proyecto desde la IDE.

---

## Fase 5 — Git avanzado  ·  M/L  ·  must + nice  ·  ✅ hecha (3.21)

**Objetivo:** pasar del "panel de stage/commit" a un flujo Git completo.
(Hecho: diff, ramas, pull, ahead/behind, descartar, quitar todo y amend.
Pendiente de esta fase: *log*/historial y *blame*.)

- `M` **Diff** del archivo seleccionado (vista unificada; lado a lado `nice`).
- `S` **Descartar cambios**, **unstage all** y **amend**.
- `M` **Ramas**: listar, crear y cambiar; mostrar *ahead/behind*.
- `S` **Pull**.
- `nice` Historial/log y *blame* básico.
- `S` Progreso/errores claros (credenciales, remoto ausente).

**Archivos:** `src/git.rs`, `ui/ide_git.slint`, `ui/ide_diff.slint` (nuevo),
`src/editor.rs`.
**Terminado:** ver qué cambió, descartar, cambiar de rama y sincronizar desde la
IDE sin romper el repo.

---

## Fase 6 — Terminal completa  ·  M  ·  must (mouse) + nice  ·  ✅ hecha (3.19)

**Objetivo:** terminal sin fricción para `vim`, la TUI de NuCoder, etc.
(Hecho: mouse reporting y sesiones renombrables/reiniciables. Pendiente de esta
fase: persistencia de sesiones/scrollback y buscar en el scrollback.)

- `M` **Mouse reporting**: reenviar el mouse al proceso cuando lo pide (el parser
  `vt100` expone los modos) y reservar la selección cuando no.
- `S` Sesiones: renombrar, cerrar, reiniciar y **cwd = proyecto** (la shell
  actual no cambia de carpeta al cambiar de proyecto).
- `M` **Persistencia** de sesiones y scrollback entre ejecuciones.
- `nice` Buscar dentro del scrollback.

**Archivos:** `src/vt.rs`, `src/editor.rs`, `ui/ide_terminal.slint`,
`ui/ide_nucoder.slint`.
**Terminado:** usar `vim`/TUI con mouse y recuperar las terminales al reabrir.

---

## Fase 7 — Flujo SpASM  ·  L  ·  must + nice  ·  ⚠️ parcial (3.22)

**Objetivo:** que NuIde sea *el* entorno de SpASM, no un editor genérico.
(Hecho: configuración de ejecución, plantillas/nuevo proyecto y ayuda de
sintaxis. **Bloqueado:** el depurador — el intérprete `spasm` no expone paso a
paso ni inspección; habría que agregarlo al toolchain en `SpAMS-Oficial`.)

- `M` **Configuración de ejecución**: archivo/entrada, argumentos, intérprete
  vs. compilado y directorio de salida (hoy usa el archivo activo).
- `M` **Nuevo proyecto** desde plantilla (wizard) y plantillas de archivo
  `.spasm`.
- `L` **Depurador**: breakpoints en el *gutter*, empezar/pausar, paso a paso,
  y vistas de **registros/memoria/variables** + consola, sobre el intérprete de
  SpASM.
- `S` Ayuda contextual de la sintaxis y firma de funciones.
- `nice` Build incremental / al guardar.

**Archivos:** `src/spasm.rs`, `src/debug.rs` (nuevo), `ui/ide_debug.slint`
(nuevo), `src/lint.rs`, `ui/ide_editor.slint`.
**Terminado:** correr y depurar un programa SpASM con breakpoints y estado
visible.

---

## Fase 8 — Ajustes y personalización  ·  M  ·  must + nice  ·  ✅ hecha (3.23)

**Objetivo:** adaptar el IDE al gusto sin editar el archivo de config a mano.
(Hecho: Preferencias de fuente/tamaño/acento/tab/compilar-al-guardar y atajos
configurables para los comandos globales, aplicados en caliente. Pendiente:
i18n y reasignar todos los atajos.)

- `M` UI de **Preferencias**: fuente y tamaño (editor y terminal), tema/acento,
  tamaño de tab, wrap, líneas del editor.
- `M` **Keybindings configurables** (mapa acción→tecla en la config).
- `S` Persistir todo en el store actual y aplicarlo en caliente.
- `nice` Idiomas (i18n) de la interfaz.

**Archivos:** `ui/ide_prefs.slint` (nuevo), `src/config.rs`, `src/editor.rs`.
**Terminado:** cambiar fuente/tema/atajos desde la UI y que sobreviva al reinicio.

---

## Fase 9 — Empaquetado y distribución  ·  M  ·  must  ·  ✅ hecha (3.24)

**Objetivo:** entregar un instalable confiable.

- `M` Recursos de Windows (icono, versión, metadatos) vía `build.rs`/`winres`.
- `S` Perfil release afinado y tamaño del binario.
- `M` **Instalador** (Inno Setup, como `SpASM-compiler.iss`) con menú inicio y
  asociación de `.spasm`.
- `S` CI publica el `.exe`/instalador como *artifact* de *tag* y un `CHANGELOG`.
- `nice` Auto-actualización (chequear *release* al arrancar).

**Archivos:** `build.rs`, `packaging/NuIde.iss` (nuevo), `.gitlab-ci.yml`,
`README.md`.
**Terminado:** instalar NuIde en una máquina limpia desde el instalador.

---

## Fase 10 — Pulido, rendimiento y documentación  ·  M  ·  must  ·  ✅ hecha (guía)

**Objetivo:** cerrar bordes y dejar documentación de usuario.

- `M` **Guía de usuario** (`docs/GUIA.md`): primer proyecto, atajos, flujo
  SpASM, terminales y Git, con capturas.
- `M` Recorrido de QA de punta a punta (checklist de los 7 puntos de v1.0).
- `M` **Rendimiento**: arranque, archivos grandes, árbol de carpetas enormes.
- `S` Accesibilidad básica: contraste, tamaños mínimos, foco visible.
- `S` Revisar `ESTADO.md` (limitaciones ya resueltas) y limpiar.

**Terminado:** un usuario nuevo completa el recorrido leyendo solo la guía.

---

## Fase 11 — Skills (Claude / OpenCode / nativos)  ·  M/L  ·  must  ·  ⚠️ parcial (S0–S3)

**Objetivo:** que NuCoder (el agente de NuIde) **descubra y cargue** skills de
los tres ecosistemas: Claude (`SKILL.md` + frontmatter), OpenCode
(`SKILL.md`/`<id>.md`, `skill` tool) y los nativos de SpASM (`skill.toml` +
`README.md`). Divulgación progresiva: el prompt sólo lleva el catálogo
(id + descripción); el cuerpo se carga con la herramienta `skill`.

**Detalle completo (arquitectura, fases S0–S7 y riesgos):**
[`SKILLS.md`](SKILLS.md).

- `M` `nucoder/src/skills.rs`: `Registry`, parseo de frontmatter YAML mínimo y
  de `skill.toml`, descubrimiento con precedencia por id.
- `S` Tool `skill` + sección **SKILLS** en el prompt de sistema + permisos
  (`allow`/`ask`/`deny`) y sandbox de lectura.
- `M` UI: modal `ui/ide_skills.slint` (catálogo, on/off, refrescar) y comando
  de paleta *Skills: abrir catálogo*.
- `nice` Menciones `@skill`, catálogos HTTP con `index.json` y shortlist de
  relevancia en SpASM.

**Archivos:** `nucoder/src/skills.rs` (nuevo), `nucoder/src/tools.rs`,
`nucoder/src/agent.rs`, `nucoder/src/config.rs`, `ui/ide_skills.slint` (nuevo),
`src/nucoder_ui.rs`, `src/editor.rs`.
**Terminado:** `nucoder skills` lista los tres orígenes, el modelo carga un
skill con la tool y el catálogo se ve en la IDE.

> Caso guía a corto plazo: **usar [REA](https://github.com/morluto/rea)**. REA
> trae un skill (`.agents/skills/reverse-engineer-anything`) **y** un MCP
> server; los skills dan el procedimiento y [`docs/MCP.md`](MCP.md) cubre las
> herramientas.

---

## Fase 12 — Cliente MCP (REA y cualquier server)  ·  L  ·  must  ·  ⚠️ parcial

> **Cliente MCP hecho** (`nucoder/src/mcp.rs` + integración al agente + CLI +
> conexión persistente `McpHub`), verificado contra **REA** (170 herramientas).
> Falta la UI (Preferencias → MCP).
>
> **M0/M1 hechos** (fuera del cliente MCP): REA 6.4.0 verificado y su **skill**
> instalado y descubierto por NuCoder (`~/.agents/skills/reverse-engineer-anything`).

**Objetivo:** que NuCoder hable **MCP** (JSON-RPC sobre stdio) para exponer las
herramientas de servers externos —empezando por **REA**— como tools nativas del
agente, con permisos.

- `M` `nucoder/src/mcp.rs`: cliente stdio (`initialize` → `tools/list` →
  `tools/call`), con server MCP de prueba en `tests/`.
- `M` Integración en `agent.rs`/`tools.rs`: inyección de tools con prefijo
  (`rea.<tool>`), ejecución enrutada, `confirm`/`deny` y truncado de salida.
- `S` Config `mcp.servers` (importando la de OpenCode/Claude) + CLI
  `nucoder mcp list|tools|call`.
- `M` UI: **Preferencias → MCP** (CRUD, probar, ver tools).
- `nice` Transporte HTTP/SSE y ciclo de vida de servidores remotos.

**Detalle completo y orden (M0–M3):** [`MCP.md`](MCP.md).
**Archivos:** `nucoder/src/mcp.rs` (nuevo), `nucoder/src/agent.rs`,
`nucoder/src/tools.rs`, `nucoder/src/config.rs`, `ui/ide_prefs.slint`,
`src/editor.rs`.
**Terminado:** `rea` como server MCP y el modelo llamando `rea.open_binary`
como herramienta nativa.

---

## Fase 13 — NuCook (destilar una habilidad)  ·  M  ·  nice  ·  ⚠️ parcial (C0, C1, C3)

**Objetivo:** usar **cualquier skill** (Claude/OpenCode/nativo) traduciéndolo a
una forma **enseñable** y **sin ruido humano**, que cualquier IA digiera rápido.
Un skill es una habilidad: si está bien enseñada, eso es todo lo que se necesita.

- `M` **C0** ✅ `nucoder/src/cook.rs`: destilador determinista (once relaciones:
  `when`, `not`, `want`, `have`, `do`, `keep`, `map`, `prove`, `eg`, `risk`,
  `rest`), render texto + JSON, y **dos guardianes**: no se pierde (`unidades ==
  conservadas + ruido + duplicados`) y no hay ruido.
- `S` CLI `nucoder cook <id|ruta> [--json|--stats]`.
- `M` **C1** ✅ caché por `hash` del origen (`~/.config/nucoder/cookies/`).
- `M` **C2** ✅ el agente anuncia la **forma enseñable** (primer de la receta) en
  vez de la descripción, y la tool `skill` devuelve la receta cocida
  (`format`: `auto`/`cooked`/`raw`).
- `M` **C3** ✅ gourmet: cocción semántica con el modelo (prompt con etiquetas +
  few-shot) y caché por `hash` + modelo + versión del prompt.
- `S` **Hub** ✅ `nucoder/src/hub.rs`: buscar (`search`), instalar
  (`install_from_dir`/`install_from_git`) y desinstalar (`uninstall`) hacia
  `~/.agents/skills`; CLI `nucook search|install|uninstall`.
- `nice` **C4** ✅ cocinar docs/KB/datasets (`nucook cook-dir` + comando de paleta
  *NuCook: cocinar carpeta de docs…*); normalizador estructural en **SpASM**
  (`spasm/cook.spasm`).
- **UI de skills** (buscar/descargar/administrar): **implementada** (U0–U4) en
  `ui/ide_skills.slint` + `src/editor.rs`; detalle en [`SKILLS_UI.md`](SKILLS_UI.md).
- `S` **Empaquetado** ✅: el instalador (`packaging/NuIde.iss`) copia
  **`nucook.exe`** (tarea `nucoder`) y `make_installer.ps1` lo compila/verifica.

**Detalle completo:** [`NUCOOK.md`](NUCOOK.md).
**Archivos:** `nucoder/src/cook.rs` (nuevo), `nucoder/src/main.rs`,
`nucoder/src/skills.rs`, `docs/NUCOOK.md`.
**Terminado:** `nucoder cook <skill>` devuelve la habilidad enseñada, sin ruido
y sin pérdida invisible.

---

## Hitos

| Hito | Fases incluidas | Resultado |
|---|---|---|
| **v0.9 — usable a diario** | 1, 2, 3 (regex/persistencia), 6 (mouse) | editar, administrar archivos, buscar y terminal con mouse |
| **v1.0 — release** | 0, 4, 5, 7, 8, 9, 10 | + buscar en proyecto, Git completo, depurador SpASM, preferencias e instalador |
| **v1.1 — agente** | 11 | + skills de Claude/OpenCode/nativos en NuCoder y la IDE |
| **v1.2 — tools externas** | 12 | + cliente MCP (REA y cualquier server) |

## Orden recomendado

Por dependencia e impacto: **0 → 1 → 2 → 3 → 6 → 4 → 5 → 7 → 8 → 9 → 10 → 11 → 12**.
Las fases 1 y 2 primero porque casi todo lo demás (menús, archivos) las usa; el
mouse de la 6 porque mejora mucho la terminal ya existente; y el depurador (7)
al final porque es lo más grande y se apoya en toolchain y editor ya pulidos.

## Riesgos y dependencias

- **Externas:** `git`, el toolchain SpASM (Python + frontend + `spasmc`) y
  `nucoder`. El IDE degrada con mensajes si faltan (ya lo hace), pero el
  depurador y build dependen de que estén.
- **`vt100`:** el mouse reporting depende de los modos que exponga el crate; si
  no alcanza, evaluar reemplazarlo o extenderlo.
- **Slint sin dependencias:** mantener la política de **cero dependencias
  innecesarias** (hoy solo `slint` + `arboard` + `rfd`); cada crate nuevo se
  justifica en el commit.
- **Empaquetado:** hoy no hay Inno Setup en la máquina de desarrollo; la fase 9
  puede requerir instalarlo o delegar el build del instalador a CI.
