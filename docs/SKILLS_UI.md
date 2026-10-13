# Interfaz de skills de NuCoder: buscar, descargar y administrar

Documento de diseño. Cómo NuCoder (el agente de NuIde) ofrece una **interfaz
para buscar skills, descargarlos y administrarlos**, y cómo se conecta con lo ya
construido: [descubrimiento](SKILLS.md), [cocción](NUCOOK.md) y el hub.

> Estado: el **núcleo** (buscar/instalar/desinstalar + cocinar/recetario), la
> **CLI `nucook`** y la **UI** (buscar/instalar/eliminar/cocinar dentro del modal
> de NuIde) están **implementados** (U0–U3). Pendiente fino: **actualizar** y
> **búsqueda remota** (catálogo HTTP / `npx skills` / GitHub).

---

## 1. Dónde vive

Dos lugares, complementarios:

- **Modal `Skills`** (ya existe: `ui/ide_skills.slint`, comando *Skills: abrir
  catálogo*): la vista de trabajo — buscar, ver, activar, cocinar, instalar,
  eliminar.
- **Preferencias → Skills** (futuro): ajustes (rutas extra, `skills_autoinvoke`,
  reglas `allow/ask/deny`, directorio del hub).

El panel de **NuCoder** (chat) ya tiene pestañas `modelos`/`permisos`; una tercera
pestaña `skills` es una alternativa válida si se prefiere todo junto al agente.
El plan asume el **modal** (más espacio y sirve aunque el agente esté cerrado).

---

## 2. Modelo de datos (una fila por skill)

```
Skill {
  id            string      // derivado de la ruta (OpenCode/Claude)
  name          string
  source        enum        // claude | agents | opencode | nativo | config | hub
  installed     bool        // vive dentro del hub (~/.agents/skills)
  state         enum        // anunciado | oculto | ask | deny
  cooked        enum        // no | deterministic | gourmet:<modelo>
  description   string
  dir           path
}
```

- **`installed`** distingue lo del hub (se puede **actualizar/eliminar**) de lo
  que viene de otros orígenes (sólo leer).
- **`cooked`** sale del recetario (`~/.config/nucoder/cookbook/<id>.cookie`).
- El resto ya lo produce `Registry` (`skills.rs`) y `cook` (`cook.rs`).

---

## 3. Buscar

### 3.1 Local (implementado)

`hub::search(&Registry, query)` filtra el catálogo **descubierto** (los 4
orígenes + el hub) por subcadena en `id`, `name` o `description`.

- En la UI: un **campo de búsqueda** que filtra las filas en vivo (el filtrado
  corre en Rust, como la paleta).
- En el CLI: `nucook search <texto>`.

### 3.2 Remoto (catálogo) — núcleo y CLI ✅, toggle en la UI ⏳

Para encontrar skills que **no** están instalados. Tres fuentes:

1. **Catálogo HTTP** (`index.json`) — **implementado**:
   `{ "skills": [ { "name", "description", "source"|"repo", "path"|"skill" } ] }`.
   `hub::search_catalog(url, query)` baja con `curl`, parsea y filtra;
   `hub::parse_catalog` tolera BOM. En el CLI:
   `nucook search <texto> --remote --catalog <url>` (o `NUCOOK_CATALOG`).
2. **`npx skills search/list`** (si hay Node): delegar al ecosistema
   (`skills.sh`). ⏳
3. **GitHub code search** (`filename:SKILL.md`): requiere token. ⏳

La UI tiene el toggle **`web`** en el modal: al prenderlo, consulta el catálogo
(en un **hilo**), agrega las entradas remotas como filas (estado `web`) y
**Instalar** las baja de su repo/path. El catálogo se toma de la variable
`NUCOOK_CATALOG` (url del `index.json`).

---

## 4. Descargar / instalar (implementado en el núcleo)

`hub::install_*` con tres entradas; todas dejan el skill en el **hub**
(`~/.agents/skills/<id>`), que `Registry` ya escanea:

| Entrada | Mecanismo | Cuándo |
|---|---|---|
| **Ruta local** (`hub::install_from_dir`) | copia recursiva | skills locales / probar |
| **Git** (`owner/repo` o URL, `hub::install_from_git`) | `git clone --depth 1` → encuentra `<skill>/SKILL.md` → copia | GitHub/GitLab |
| **`npx skills add`** (fase U2) | proceso hijo | ecosistema `skills.sh` |

Detección de la carpeta del skill en el repo: `.agents/skills/<id>`,
`.opencode/skills/<id>`, `.claude/skills/<id>`, `skills/<id>`; si no hay `--skill`,
el **primer** `SKILL.md` que aparezca.

CLI: `nucook install <repo|ruta> [--skill <id>] [--force]`.

---

## 5. Administrar

| Acción | Qué hace | Dónde | Estado |
|---|---|---|---|
| **Buscar** | filtra la lista en vivo (Rust) | modal (campo) | ✅ |
| **Ver** | muestra la receta (cooked) o el original | modal (detalle) | ✅ |
| **Cocinar** | `cook_skill_gourmet` + `save_cookie` (en hilo) | modal (botón) | ✅ |
| **Instalar** | `hub::install_*` (en hilo) | modal (botón + `IdePrompt`) | ✅ |
| **Actualizar** | reinstala (`install --force`) | CLI (`--force`) | ⏳ UI |
| **Eliminar** | `hub::uninstall` (sólo si `installed`) | modal (confirmación) | ✅ |
| **Cocinar todas** | `nucook bake` → recetario | CLI | ⏳ UI |

El **recetario** (`cookbook`) es lo que hace que el agente anuncie la **forma
enseñable** en vez de la descripción (C2). Cocinar una skill mejora la decisión
del agente sin cambiar el mecanismo.

---

## 6. La interfaz (boceto)

```
┌─ Skills ─────────────────────────────────────────────── [🔍 buscar…] ─┐
│ ┌ lista ────────────────────────────┐ ┌ detalle ─────────────────────┐ │
│ │ ● reverse-engineer-anything  allow │ │ nucook/v1 · gourmet:nvidia…  │ │
│ │   [agents] REA · receta ✓          │ │ skill reverse-engineer-any…  │ │
│ │ ● spasm-architect            allow │ │ want explain a feature …     │ │
│ │   [opencode] KB del lenguaje       │ │ not   target is a source…    │ │
│ │ ○ pty                        deny  │ │ map   asar -> analyze_js…    │ │
│ │   [nativo] PTY (skeleton)          │ │ keep  cite Evidence IDs      │ │
│ │                                    │ │ …                            │ │
│ └────────────────────────────────────┘ └──────────────────────────────┘ │
│ [Anunciar/ocultar] [Cocinar] [Instalar…] [Eliminar] [Refrescar]         │
└─────────────────────────────────────────────────────────────────────────┘
```

- **Buscar**: campo arriba; filtra la lista (Rust).
- **Instalar…**: diálogo de texto (`IdePrompt`) con `owner/repo` o una ruta;
  opcional `--skill`. Corre en un **hilo** (el `git clone` puede tardar) y avisa
  al terminar; luego refresca la lista.
- **Cocinar**: corre el pase gourmet (o determinista) y guarda en el recetario;
  en hilo + caché. Un ícono en la fila marca `receta ✓` cuando está cocida.
- **Eliminar**: `IdeAsk` de confirmación; sólo para filas `installed`.
- **Detalle**: la receta (si existe) o el cuerpo; con la ruta base y recursos.

Reutiliza lo que ya está: `IdePrompt` (texto), `IdeAsk` (confirmación), el patrón
de “refresco acotado” (sólo si el modal está abierto) y los canales por hilo.

---

## 7. Seguridad

Un skill son **instrucciones**: puede pedirle al agente que borre, suba o ejecute
cosas.

- **Instalar no ejecuta nada**: sólo copia `SKILL.md` + recursos.
- **Nunca** se corren scripts del skill al instalar ni al cocinar.
- Al usar el agente, el skill pasa por la **confirmación** normal
  (`confirm`/auto-aprobar) y por los permisos `allow/ask/deny`.
- La UI **muestra el cuerpo** antes de activar; instalar/enlazar es explícito.
- `uninstall` sólo borra dentro del hub (`~/.agents/skills`), con id validado.
- Origen visible por fila (`claude`/`opencode`/`nativo`/`hub`) para saber de
  dónde salió.

---

## 8. Fases de la UI

| Fase | Qué | Reusa |
|---|---|---|
| **U0** ✅ | Modal de skills: lista, origen, estado, detalle, **Anunciar/ocultar**, Refrescar | `ide_skills.slint` + `editor.rs` |
| **U1** ✅ | **Buscar**: campo de filtrado en vivo sobre la lista | `hub::search` (lógica) + `skills_filter` |
| **U2** ✅ | **Instalar/Actualizar/Eliminar**: `IdePrompt` + hilos (`git`), `IdeAsk` para borrar | `hub::install_*`, `hub::uninstall` |
| **U3** ✅ | **Cocinar** desde la UI (detalle con la receta, marca cocida) | `cook`, `cookbook` |
| **U4** ✅ | Toggle **`web`**: buscar en el catálogo remoto e instalar desde la lista | `hub::search_catalog` |

> Pendiente fino: fuentes remotas **`npx skills`** y **GitHub code search**
> (además del catálogo HTTP), y **cocinar todas** desde la UI.

### Cómo se implementa U1–U3 (guía)

1. **Estado** en `Ide` (`editor.rs`): `skills_filter: String`,
   `skills_busy: bool`, y un canal `skills_tx/rx` para instalar/cocinar en hilo.
2. **Slint** (`ui/ide_skills.slint`): agregar `filter` (TextInput) + botones; el
   modal ya tiene el layout de lista/detalle y el patrón de estado.
3. **Callbacks**: `skills-install` (abre `IdePrompt` → `hub::install_*` en hilo),
   `skills-cook`, `skills-delete` (`IdeAsk` → `hub::uninstall`), `skills-search`.
4. **Refresco**: al terminar una operación, `skills_reload()` reconstruye el
   `Registry` (que ya incluye el hub) y las filas.
5. **Descubrimiento**: el hub (`~/.agents/skills`) ya está en las raíces de
   `Registry::discover`, así que lo instalado aparece sin tocar el descubrimiento.

---

## 9. Qué ya está (por debajo de la UI)

- `nucoder/src/skills.rs` — descubrimiento de los 4 orígenes + permisos.
- `nucoder/src/cook.rs` — cocción (determinista y gourmet) + **recetario**.
- `nucoder/src/hub.rs` — **buscar** (`search`), **instalar**
  (`install_from_dir`/`install_from_git`) y **desinstalar** (`uninstall`).
- `nucoder/src/bin/nucook.rs` — CLI: `list`, `catalog`, `search`, `install`,
  `uninstall`, `bake`, `show`, `cook`.
- `ui/ide_skills.slint` + `src/editor.rs` — modal (U0).

La UI (buscador, instalar/eliminar/cocinar) es **cablear** esos servicios a los
botones del modal, con hilos para lo que toca la red.
