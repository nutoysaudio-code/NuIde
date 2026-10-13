# Skills en NuIde / NuCoder — plan de trabajo

**Respuesta corta: sí, es posible**, y encaja casi sin fricción con lo que ya
existe. NuCoder ya tiene un **índice de conocimiento local** (`src/knowledge.rs`)
que descubre documentos y los expone con la herramienta `read_docs` más un
*primer* en el prompt de sistema. Los *skills* son exactamente el mismo patrón,
pero con **metadata + carga por demanda** (divulgación progresiva), así que se
extiende ese módulo en vez de inventar un sistema nuevo.

> Documento de trabajo. El estado de NuIde vive en [`ESTADO.md`](ESTADO.md), el
> plan general en [`PLAN.md`](PLAN.md) y la arquitectura del agente en
> [`../nucoder/DESIGN.md`](../nucoder/DESIGN.md) y
> [`../nucoder/README.md`](../nucoder/README.md).

---

## 1. Los tres formatos que hay que entender

Ya están en la máquina los tres, y **dos comparten formato**:

| Origen | Archivo de entrada | Metadata | Dónde vive | Carga |
|---|---|---|---|---|
| **Claude** | `SKILL.md` | frontmatter YAML (`name`, `description`, `allowed-tools`) | `~/.claude/skills/<id>/`, `<proyecto>/.claude/skills/<id>/` | `SKILL.md` + recursos del directorio |
| **OpenCode** | `SKILL.md` o `<id>.md` en la raíz del origen | frontmatter YAML (`name`, `description`, `metadata.opencode/autoinvoke`, `disable-model-invocation`, `allowed-tools`) | `~/.config/opencode/skills/`, `.opencode/skills/`, `~/.agents/skills/`, `.agents/skills/` + `skills` del config | `skill` tool con el id: cuerpo sin frontmatter + dir base + hasta 10 recursos |
| **Nativo SpASM** | `README.md` (+ `skill.toml`) | TOML (`name`, `version`, `status`, `owner`, `entrypoint_doc`, `compat`, `capabilities`, `dependencies`, `notes`) | `F:\SpAMS-Oficial\skills\<id>\` (o `SPASM_HOME\skills\`) + `index.json` | `README.md` / los `.spasm` del skill |

Evidencia real de cada formato en esta máquina:

- Claude: `C:\Users\quamagi\.claude\skills\spasm-architect\SKILL.md` (frontmatter
  `name`/`description`).
- OpenCode: `C:\Users\quamagi\.config\opencode\skills\spasm-architect\SKILL.md`
  (mismo formato: **mismo parser**).
- Nativo: `F:\SpAMS-Oficial\skills\spasm-architect\{skill.toml, README.md}` y el
  catálogo `F:\SpAMS-Oficial\skills\index.json`.

**Conclusión de diseño:** un solo `Registry` con un parser de *frontmatter
YAML mínimo* (para SKILL.md) y un parser de *TOML mínimo* (para skill.toml),
porque el proyecto tiene **política de cero dependencias innecesarias**
(`serde_yaml`/`toml` no se justifican para leer 4 campos).

---

## 2. Cómo funciona OpenCode (referencia a respetar)

Del doc oficial (`https://opencode.ai/v2/docs/skills/`), lo que hay que imitar:

1. **Descubrimiento** con precedencia (menor → mayor): built-ins → `.claude/skills`
   (global y ancestros) → `.agents/skills` → `~/.config/opencode/skills` →
   `.opencode/skills` (raíz → cwd) → entradas `skills` del config.
2. **Formas aceptadas**: `<origen>/<id>.md` (raíz) o `<origen>/<id>/SKILL.md`
   (en cualquier profundidad). El **id lo da la ruta**, no el frontmatter; es
   sensible a mayúsculas.
3. **Divulgación progresiva**: al modelo sólo se le muestra `id`, `name` y
   `description`. El cuerpo se carga **cuando el modelo llama a la herramienta
   `skill`** (`{ "id": "git-release" }`).
4. Al cargar: se agrega el **cuerpo sin frontmatter**, se informa el **directorio
   base** y una **muestra de hasta 10 rutas de archivos de apoyo** (no se cargan
   solos: el modelo los lee si el skill se lo indica).
5. **Permisos** por id (`allow` / `ask` / `deny`) y `opencode/autoinvoke: false`
   para ocultar del listado sin dejar de poder cargarlo explícitamente.
6. **Catálogos HTTP** con `index.json` (opcional, fase tardía).

El skill que ve el agente en este entorno llega justamente por esa vía
(`spasm-architect`).

---

## 3. Arquitectura propuesta

```
                 nucoder/src/skills.rs  (nuevo)
  ┌─────────────────────────────────────────────────────────────┐
  │  SkillSource = Claude | Agents | OpenCode | Native | Config  │
  │  Skill { id, name, description, dir, entry, resources,       │
  │          autoinvoke, allowed_tools, source, capabilities }   │
  │  Registry::discover(cwd, extra)   → precedencia por id       │
  │  Registry::advertised()           → id+name+description      │
  │  Registry::load(id)               → cuerpo + dir + recursos  │
  └───────────────┬─────────────────────────────┬───────────────┘
                  │                             │
        agent.rs (system_prompt)          tools.rs (tool `skill`)
        catálogo breve                    carga por demanda
                  │                             │
             prompt de sistema  ────────►  conversación
                  │
        ui/ide_skills.slint + nucoder_ui.rs  (catálogo, on/off, refrescar)
```

Convive con la KB actual:

- **KB (`read_docs`)**: documentos de dominio (SpASM, quickref, specs). Se queda.
- **Skills (`skill`)**: *procedimientos/instrucciones* reutilizables. Nuevo.
- Un skill **nativo** apunta su entrada a `README.md`; Claude/OpenCode, a
  `SKILL.md`. El mismo `load()` sirve para los tres.
- Los directorios de skills se agregan a las **raíces permitidas** del sandbox
  (igual que hoy se permite leer la KB fuera del proyecto).

---

## 3.5 Caso de uso: usar REA (objetivo a corto plazo)

[REA](https://github.com/morluto/rea) (*Reverse Engineer Anything*) es el caso
concreto que empuja este plan. Trae **dos piezas**:

- un **agent skill** `.agents/skills/reverse-engineer-anything/SKILL.md` (con
  frontmatter `name`/`description`/`metadata.version` y una carpeta
  `references/` de 6 documentos) — **esto lo cubre este plan**;
- un **MCP server** (`npx -y rea-agents@6.4.0 mcp`) con las herramientas — eso
  lo cubre [`MCP.md`](MCP.md), porque **NuCoder todavía no habla MCP**.

Es decir: los skills le dan al agente el **procedimiento** (cuándo usar REA y
qué tool por target), pero *“instalar el skill no registra el server ni agrega
tools”* (lo dice el propio REA). Para “usar REA” de verdad hay que sumar el
cliente MCP; mientras tanto, REA se usa por su **CLI** desde la terminal.

Lo que este plan aporta a REA, en concreto:

- descubrir `.agents/skills` (global `~/.agents/skills` y del proyecto), que es
  donde REA instala el skill compartido;
- cargar el cuerpo **y los `references/`** como recursos del skill (el SKILL.md
  remite a `references/connection-and-recovery.md`, etc.);
- `allow`/`ask`/`deny` por skill, para decidir si se anuncia y si pide permiso.

Ver el detalle y el orden en [`MCP.md`](MCP.md) §1 y §3 (fases M0–M3).

---

## 3.6 Buscar, descargar y administrar (hub + UI)

Para **usar cualquier skill** hace falta poder encontrarlo, bajarlo y
administrarlo. El **núcleo** ya está:

- `nucoder/src/hub.rs`: **buscar** (`search`), **instalar**
  (`install_from_dir` / `install_from_git` con `owner/repo` o URL) y
  **desinstalar** (`uninstall`), con el destino compartido `~/.agents/skills`
  (que `Registry` ya escanea).
- CLI `nucook search|install|uninstall` (ver [`NUCOOK.md`](NUCOOK.md)).
- Cocción y **recetario** para que el agente anuncie la **forma enseñable**.

La **interfaz** (buscador en vivo, botones instalar/cocinar/eliminar en el modal
de skills, y búsqueda remota opcional) está diseñada en
[`SKILLS_UI.md`](SKILLS_UI.md), con las fases U0–U3.

---

## 4. Fases

Leyenda de tamaño: `S` (horas), `M` (1–2 días), `L` (varios días).
Prioridad: `must` (sin esto la compatibilidad no existe) / `nice` (mejora).

> **Estado:** S0, S1, S2, S3 y S4 **implementadas** (`nucoder/src/skills.rs`, tool
> `skill`, `nucoder skills|skill`, `--skill`, menciones `@id`, permisos
> `allow/ask/deny` + `skills_autoinvoke`, y el catálogo visual
> `ui/ide_skills.slint`); **hub** de buscar/instalar/desinstalar
> (`nucoder/src/hub.rs` + `nucook search|install|uninstall`) implementado; UI de
> buscar/descargar/administrar diseñada en [`SKILLS_UI.md`](SKILLS_UI.md). S5/S6
> (nice) pendientes.

### Fase S0 — Núcleo: descubrimiento y parseo  ·  M  ·  must  ·  ✅ hecha

**Objetivo:** un `Registry` que encuentre los tres formatos y los normalice.

- `S` Struct `Skill` + enum `SkillSource` + `Registry` en
  `nucoder/src/skills.rs`; agregarlo a `lib.rs`.
- `M` **Parser de frontmatter YAML mínimo**: bloque `---…---`, claves planas
  (`name`, `description`, `allowed-tools`), `disable-model-invocation`, y el
  sub-bloque `metadata:` con `opencode/autoinvoke`. Tolerante: si algo no
  parsea, el skill se indexa con lo que sí se pudo leer.
- `S` **Parser de TOML mínimo** para `skill.toml`: `name`, `entrypoint_doc`,
  `compat`, `capabilities = [...]`, `dependencies`, `notes`.
- `M` **Descubrimiento** con la precedencia de §2: `.claude`, `.agents`,
  `.config/opencode`, `.opencode` (raíz → cwd), y **nativo**
  (`SPASM_HOME/skills` o `F:\SpAMS-Oficial\skills`, leyendo `index.json` si
  está). Override por id: gana el de mayor precedencia.
- `S` `advertised()` (sólo con `description` y sin autoinvoke off), `by_id()`,
  `load()`, `catalog_prompt()`.
- `M` Tests: frontmatter con/sin `metadata`, forma plana `<id>.md`, forma
  `<id>/SKILL.md`, id derivado de la ruta, `skill.toml`, precedencia/override,
  ausencia de `description` ⇒ no se anuncia.

**Archivos:** `nucoder/src/skills.rs` (nuevo), `nucoder/src/lib.rs`.
**Terminado:** un test descubre los tres formatos en directorios temporales y
`load()` devuelve el cuerpo sin frontmatter + recursos.

### Fase S1 — Exponer al agente  ·  M  ·  must  ·  ✅ hecha

**Objetivo:** el modelo puede ver y cargar skills.

- `S` **Tool `skill`** (`{ id }`) en `tools.rs` + `tool_specs()`; devuelve el
  cuerpo, el directorio base y hasta 10 rutas de apoyo (como OpenCode).
- `S` `Ctx` gana `skills: Option<&Registry>`; `Agent` lo construye en `run()`.
- `S` `system_prompt()` agrega la sección **SKILLS** con el catálogo
  (`id — name: description`), sólo si hay; y una línea que explique que se
  cargan con la herramienta `skill` (no inventar skills).
- `S` El sandbox permite **leer** el directorio de cada skill (aunque esté
  fuera del proyecto) y sus recursos.
- `S` CLI: `nucoder skills` (lista), `nucoder skill <id>` (imprime el cuerpo),
  `--skill <id>` (precarga como primer turno).
- `M` Tests: la tool devuelve el cuerpo sin frontmatter; un skill sin
  `description` no aparece en el catálogo; `load` de id inexistente es error.

**Archivos:** `nucoder/src/tools.rs`, `nucoder/src/agent.rs`,
`nucoder/src/main.rs`.
**Terminado:** `nucoder skills` lista los tres orígenes y `nucoder skill
spasm-architect` imprime el cuerpo.

### Fase S2 — Permisos y configuración  ·  S/M  ·  must  ·  ✅ hecha

**Objetivo:** controlar qué skills se anuncian/cargan y de dónde salen.

- `S` Config nueva: `skills` (rutas/dirs extra, como `knowledge`) y
  `skills_permissions` (`[{id_glob, effect: allow|ask|deny}]`, semántica de
  OpenCode: la última regla que matchea gana). Persistir en
  `~/.config/nucoder/config.json` (mismo `save_own`).
- `S` `deny` ⇒ no se anuncia y `load()` la rechaza; `ask` ⇒ pasa por el flujo
  de `Confirm`/`auto_approve` existente.
- `S` `NUCODER_SKILLS` (rutas separadas por el separador del SO) y un toggle
  `skills_autoinvoke` global.
- `S` Tests: deny oculta y rechaza; ask dispara confirmación; allow carga
  directo; el glob se evalúa en orden.

**Archivos:** `nucoder/src/config.rs`, `nucoder/src/skills.rs`,
`nucoder/src/agent.rs`, `nucoder/src/main.rs`.
**Terminado:** un id en `deny` no aparece ni carga; en `ask` respeta
auto-aprobar.

### Fase S3 — UI en NuIde  ·  M  ·  must  ·  ✅ hecha

**Objetivo:** ver y manejar los skills sin editar archivos.

- `M` Modal `ui/ide_skills.slint` (patrón de `ui/ide_ftp.slint` /
  `ui/ide_sqlite.slint`): lista con **id**, **name**, **descripción** y
  **origen** (badge Claude/OpenCode/Nativo), filtro por texto, botón
  **Refrescar** y un **detalle** que muestra el cuerpo (Markdown) al
  seleccionar.
- `S` Toggle **anunciar al agente** por skill (escribe
  `skills_permissions`, `autoinvoke`).
- `S` Cableado: `src/nucoder_ui.rs` + `src/editor.rs` (estado, canal,
  drenado), comando de paleta *Skills: abrir catálogo* y entrada en el menú
  **Agente**. Refresco al cambiar de proyecto.
- `S` i18n: los textos nuevos entran al catálogo `lang/*/nuide.po`
  (ver `docs/I18N.md`).

**Decisión abierta:** modal (recomendado, menos invasivo, como FTP/SQLite) vs.
pestaña dentro de Preferencias vs. dock propio. El plan asume **modal**.
**Hecho como modal** (`ui/ide_skills.slint`), con comando de paleta *Skills:
abrir catálogo* y entrada en el menú **Agente**.
**Archivos:** `ui/ide_skills.slint` (nuevo), `ui/nuide_ui.slint`,
`ui/ide_types.slint`, `src/editor.rs`, `lang/**/nuide.po`.
**Terminado:** abrir el catálogo desde la paleta, refrescar y prender/apagar
un skill, y que sobreviva al reinicio.

### Fase S4 — Menciones `@skill` en el chat  ·  S  ·  nice  ·  ✅ hecha

**Objetivo:** forzar un skill desde el input de NuIde, como OpenCode.

- `S` Detectar `@<id>` en el turno del panel y **precargar** el cuerpo de ese
  skill (chip visible), sin esperar a que el modelo llame a la herramienta.
- `S` Autocompletado mínimo de ids al tipear `@`.
- `S` Test: `@id` inexistente avisa; `@id` de un `deny` no se precarga.

**Archivos:** `src/nucoder_ui.rs`, `nucoder/src/agent.rs`,
`ui/ide_nucoder.slint`.
**Terminado:** escribir `@spasm-architect` inserta su contenido en el turno.

### Fase S5 — Catálogos (`index.json`) y HTTP  ·  M  ·  nice

**Objetivo:** catálogos remotos + el índice nativo.

- `S` Leer el `index.json` **nativo** de SpASM como catálogo (nombre → ruta),
  ya local.
- `M` **Catálogos HTTP** (`skills` con `http(s)://`): bajar `index.json`,
  validar rutas (relativas, mismo origen) y cachear por `version` (con
  `curl.exe`, como `web_fetch`; sin crates de red — coherencia con §3.27 del
  `ESTADO.md`).
- `S` Tests con un `index.json` fixture local y con un servidor de prueba
  (`#[ignore]`, patrón de `tests/webserver_e2e.rs`).

**Archivos:** `nucoder/src/skills.rs`, `nucoder/src/proc.rs`,
`nucoder/tests/`.
**Terminado:** un catálogo HTTP se descubre, cachea y carga un skill.

### Fase S6 — Shortlist de skills en SpASM  ·  S/M  ·  nice

**Objetivo:** coherencia con el ecosistema: la **decisión** en SpASM.

- `M` `nucoder/spasm/skills.spasm` (AOT → C → FFI, como `router.spasm`):
  recibe la **tarea** y la lista `id;keywords` y devuelve los índices de los
  skills más relevantes, para anunciar sólo esos cuando el catálogo sea grande.
- `S` `ffi/skills_ffi.c` + `src/ffi.rs` (mutex, buffers estáticos) y
  `tools/regen_spasm.ps1`; C generado commiteado.
- `M` Tests de paridad en `src/skills.rs` y actualización de
  `nucoder/spasm/README.md` y `nucoder/DESIGN.md`.

**Archivos:** `nucoder/spasm/skills.spasm`,
`nucoder/spasm/generated/*.c`, `nucoder/ffi/skills_ffi.c`,
`nucoder/src/ffi.rs`, `nucoder/src/skills.rs`.
**Terminado:** con > N skills, el catálogo del prompt trae sólo los relevantes
a la tarea.

### Fase S7 — Documentación y CI  ·  S  ·  must

- `S` `docs/SKILLS.md` (este documento) + entrada en el `README.md` y una fase
  en `docs/PLAN.md` + `nucoder/README.md` (sección *Skills*).
- `S` `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` en los dos
  crates; `slint-viewer --check ui/nuide_ui.slint`; test guardián de que no se
  versionan secretos (ya existe para providers) y de que no se edita el C
  generado a mano.

**Terminado:** guía + CI en verde.

---

## 5. Orden recomendado

`S0 → S1 → S2 → S3 → S7` cubren la **compatibilidad real** (leer y usar los
tres formatos desde la IDE). `S4`, `S5` y `S6` son mejoras.

| Hito | Fases | Resultado |
|---|---|---|
| **Compat. mínima** | S0, S1, S2, S7 | NuCoder descubre, anuncia y carga skills de Claude/OpenCode/nativos |
| **Compat. usable** | + S3 | catálogo visual en la IDE |
| **Paridad OpenCode** | + S4, S5 | menciones `@skill` y catálogos HTTP |
| **Ecosistema** | + S6 | la relevancia se decide en SpASM |

---

## 6. Riesgos y decisiones

- **YAML/TOML sin crates.** El frontmatter documentado es chico; un parser
  mínimo alcanza. Si un skill usa YAML avanzado, se degrada con aviso en vez de
  fallar. (Si algún día hace falta, `serde_yaml`/`toml` se justifica en el
  commit, como manda `PLAN.md`.)
- **Seguridad.** Un skill son *instrucciones*: puede pedirle al agente que
  borre, suba o ejecute cosas. **No** se ejecutan scripts de un skill por el
  solo hecho de cargarlo; el modelo los corre con las tools normales, que ya
  pasan por confirmación/auto-aprobar. `deny` no es cosmético: oculta y rechaza.
- **Presupuesto de tokens.** Sólo se anuncian `id`+`name`+`description`. Con
  muchos skills, la Fase S6 (shortlist en SpASM) acota el catálogo.
- **IDs duplicados.** Se resuelven por precedencia (gana el de mayor); es
  intencional, para poder pisar un skill global con uno del proyecto.
- **Sandbox.** Los recursos viven fuera del proyecto (`~/.claude`, etc.): se
  agregan como raíces de lectura permitidas, sin habilitar escritura.
- **Nativo sin `SKILL.md`.** El skill nativo es `skill.toml` + `README.md`; su
  `entrypoint_doc` puede apuntar a otro `.md`. Se soporta como entrada.
- **`read_docs` vs `skill`.** Se mantienen los dos: `read_docs` para la KB de
  dominio y `skill` para los procedimientos. Eventualmente un skill puede
  declarar documentos de KB propios.
```
