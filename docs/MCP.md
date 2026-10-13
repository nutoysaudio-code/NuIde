# MCP en NuCoder — plan de trabajo (caso guía: REA)

**Objetivo a corto plazo:** poder **usar [REA](https://github.com/morluto/rea)**
(Reverse Engineer Anything) desde NuIde/NuCoder.

REA son **dos piezas separadas** que hay que tratar distinto:

| Pieza | Qué aporta | Cómo se usa | ¿NuCoder hoy? |
|---|---|---|---|
| **Skill** `reverse-engineer-anything` | Las *instrucciones*/flujo de trabajo: cuándo usar REA, qué tool por target, cómo citar evidencia | `SKILL.md` + `references/` en `.agents/skills` o `~/.claude/skills` | ❌ (lo cubre [`SKILLS.md`](SKILLS.md)) |
| **MCP server** | Las *herramientas* (`open_binary`, `analyze_javascript_application`, `inspect_managed_artifact`, …) | JSON-RPC por **stdio**: `npx -y rea-agents@6.4.0 mcp` | ❌ **NuCoder no habla MCP** |

> El propio skill lo aclara: *"Installing this skill supplies instructions; it
> does not register the REA MCP server … or add tools to an already-running
> agent."* Y: *"Prefer an existing `rea` CLI."*

Conclusión: hay **tres caminos** para “usar REA”, de menos a más esfuerzo.

> **Estado (M0, M1 y núcleo de M2 hechos):** se verificó **REA 6.4.0** con Node
> 26; el **skill** se instaló en `~/.agents/skills/reverse-engineer-anything/`
> (con sus `references/`) y NuCoder lo descubre y lo anuncia. El **cliente MCP**
> ya está: `nucoder/src/mcp.rs` (JSON-RPC por stdio), integrado al agente
> (`<server>__<tool>`, confirmación por server, filtro `tools`) y con CLI
> (`nucoder mcp list|tools|call`). Las conexiones son **persistentes** entre
> turnos (`McpHub`). Verificado contra REA: **170 herramientas** listadas. Falta
> la **UI** (Preferencias → MCP).

---

## 0. Camino inmediato: la CLI (0 código)

Hoy mismo se puede usar REA desde NuIde **sin tocar NuCoder**:

- Terminal de la IDE (o el menú **Ejecutar**) en un proyecto:
  ```powershell
  npx -y rea-agents@latest doctor --json          # chequeo de entorno
  npx -y rea-agents@latest analyze-javascript-application "C:/ruta/app" --json
  npm install --global rea-agents                 # deja `rea` en el PATH
  ```
- El agente ya puede pedirlo con su herramienta **`bash`**
  (`npx -y rea-agents@latest …`) o **`run_in_terminal`** para corridas largas.
  REA tiene herramientas “target-free” que **no** necesitan Hopper/Ghidra/IDA
  (JS/Electron estático, .NET, Go, WASM, SQLite, PE resources, APKs, archivos).

**Limitación honesta (Windows):** Hopper **no** existe en Windows; el soporte
Ghidra en Windows es **P0 experimental**; IDA es comercial. En Windows, REA
sirve sobre todo para análisis **estático** (JS/Electron, .NET, Go, WASM,
SQLite, PE, paquetes), no para decompilar binarios nativos como en macOS/Linux.

Este camino **no da** las tools como herramientas nativas del agente: el modelo
tiene que invocar la CLI a mano (y parsear su JSON). Es el “usarlo ya”, no la
integración.

---

## 1. Por qué hace falta MCP

`nucoder/src/tools.rs` define las herramientas **a fuego** (`tool_specs()`).
El bucle del agente (`src/agent.rs`) sólo sabe ejecutar esos nombres
(`tools::execute`). Todo lo demás —REA, o cualquier otro servidor MCP— es
invisible para el modelo.

Para que el agente vea `open_binary`, `analyze_javascript_application`, etc.,
hay que agregar un **cliente MCP** que:

1. arranque el server como proceso hijo y hable **JSON-RPC 2.0 sobre stdio**;
2. haga `initialize` → `notifications/initialized` → `tools/list`;
3. **inyecte** esas tools en la lista que se le manda al modelo, y
4. enrute sus llamadas a `tools/call`, devolviendo el contenido como texto.

REA publica el server como **paquete npm** y en el **MCP Registry**
(`io.github.morluto/rea`): `npx -y rea-agents@6.4.0 mcp` (stdio).

---

## 2. Arquitectura propuesta

```
nucoder/src/mcp.rs  (nuevo)
┌──────────────────────────────────────────────────────────────┐
│ struct McpServer { id, command, args, env, enabled, confirm } │
│ struct McpClient { child, rx/tx de JSON-RPC, tools: Vec<Tool>}│
│   spawn()   → initialize / initialized / tools/list           │
│   call()    → tools/call { name, arguments } → texto          │
│   tools()   → [{ name, description, inputSchema }]            │
└───────────────┬──────────────────────────────┬───────────────┘
                │                              │
        agent.rs (tool_specs)            tools.rs (execute)
        tools del server, con prefijo    enruta <server>.<tool>
```

- **Transporte**: stdio (el caso de REA) **primero**; HTTP/SSE después.
  Reusar `crate::proc::hidden` para no abrir consola.
- **Enmarcado**: MCP stdio usa **JSON por línea** (no `Content-Length`).
- **Namespacing**: prefijo `rea.open_binary` (configurable) para evitar colisión
  con las tools propias; el prefijo también identifica el server al ejecutar.
- **Resultado**: `tools/call` devuelve `content: [{type:text|image|resource}]`.
  MVP: concatenar los `text`; las imágenes se pueden adjuntar en una fase
  siguiente (NuIde ya soporta imágenes en el chat).
- **Presupuesto**: REA devuelve resultados grandes y tiene *Evidence IDs*
  paginables; el cliente debe respetar un tope de bytes y ofrecer
  `inspect_analysis_view`-style paginado **vía el propio server** (no inventar).

### Permisos

- Por server: `enabled`, `confirm` (pide aprobación en cada `tools/call`) y
  `allow`/`deny` por tool (glob). Se integra con el flujo existente
  `Confirm` / `auto_approve` / autopiloto.
- REA **no requiere** permisos por su lado, pero NuCoder puede exigir
  aprobación porque muchas tools **ejecutan** el target o lanzan motores
  (Hopper/Ghidra) — eso es acción sensible.
- Un server `deny` no se anuncia; `ask` pasa por `Confirm`.

### Config

En `~/.config/nucoder/config.json` (misma `save_own`), compatible con la forma
de OpenCode/Claude para poder **importar**:

```json
"mcp": {
  "servers": {
    "rea": {
      "command": "npx",
      "args": ["-y", "rea-agents@6.4.0", "mcp"],
      "env": {},
      "enabled": true,
      "confirm": true
    }
  }
}
```

---

## 3. Fases

### Fase M0 — REA por CLI (camino 0)  ·  S  ·  must  ·  ✅ hecha
- `S` Probar `npx rea-agents@latest doctor --json` en el proyecto y documentar
  los resultados en Windows (qué motores hay, qué tools target-free sirven).
- `S` Guía corta en `docs/GUIA.md`: cómo correr REA desde la terminal/`bash`.

**Terminado:** una investigación real hecha con REA por CLI, sin código nuevo.

### Fase M1 — Skills de REA (habilita el “cuándo/cómo”)  ·  S/M  ·  must  ·  ✅ hecha
Depende de [`SKILLS.md`](SKILLS.md) Fases S0–S1. El skill de REA es la forma
`.agents/skills/<id>/SKILL.md` **con carpeta `references/`** (6 documentos).
- `S` Incluir `.agents/skills` (global y de proyecto) en el descubrimiento.
- `S` Soportar **recursos** (`references/*.md`): el `load()` devuelve el cuerpo
  + hasta 10 rutas, y `read_file` permite leerlos (raíz de lectura del skill).
- `S` Instalar el skill con `npx skills add morluto/rea --skill
  reverse-engineer-anything` (o `rea setup --skill-client …`) y verificar que
  NuCoder lo anuncia y lo carga.

**Terminado:** el agente anuncia `reverse-engineer-anything`, lo carga cuando la
tarea es de ingeniería inversa y opera REA por la CLI `rea` con criterio.

### Fase M2 — Cliente MCP (integración real)  ·  L  ·  must  ·  ⚠️ parcial (núcleo + CLI; falta UI)
- `M` `nucoder/src/mcp.rs`: cliente stdio (spawn, `initialize`, `tools/list`,
  `tools/call`), con tests contra un server MCP de prueba (fixture en `tests/`).
- `M` Integración en `agent.rs`/`tools.rs`: inyección de tools con prefijo,
  ejecución enrutada, `confirm`/`deny`, truncado de salida.
- `S` Config `mcp.servers` + import de configs de OpenCode/Claude.
- `S` CLI: `nucoder mcp list`, `nucoder mcp tools <server>`,
  `nucoder mcp call <server> <tool> --json`.
- `M` UI: **Preferencias → MCP** (alta/edición/borrado, probar, ver tools) y
  entrada en la paleta; i18n.
- `S` Ciclo de vida: arrancar los servers al iniciar el agente, reiniciar al
  cambiar de proyecto, y apagarlos al salir (evitar huérfanos de `npx`).

**Terminado:** `rea` aparece como server MCP; el modelo llama `rea.open_binary`
y compañía como herramientas nativas, con permisos y sin consolas parpadeando.

### Fase M3 — HTTP/SSE y MCP de OpenCode  ·  M  ·  nice
- `M` Transporte HTTP/SSE para servers remotos, y consumo del `mcp` de
  `opencode.json` para reaprovechar la configuración existente.

---

## 4. Orden recomendado y resultado

```
M0 (hoy)  →  M1 (skills, junto a S0–S1)  →  M2 (cliente MCP)  →  M3 (nice)
```

| Hito | Fases | Resultado |
|---|---|---|
| **REA usable ya** | M0 | investigaciones con REA por CLI desde la IDE |
| **REA entendido** | + M1 | el agente sabe cuándo/cómo usar REA y lo maneja por CLI |
| **REA integrado** | + M2 | las tools de REA dentro del bucle del agente, como en Claude Code/OpenCode |
| **MCP completo** | + M3 | cualquier server MCP (stdio y HTTP), config compartida |

---

## 5. Riesgos y decisiones

- **Sin crates nuevas.** JSON-RPC se hace con `serde_json` (ya está) + `std`;
  el proceso hijo con `proc::hidden`. Coherente con la política del proyecto.
- **Procesos huérfanos.** `npx` deja hijos; hay que cerrar el árbol al salir y
  al reiniciar (Job Objects en Windows, como ya hace REA).
- **Respetar el contrato del server.** No adivinar schemas: se usan los que
  devuelve `tools/list`. REA advierte que el skill de `main` puede describir
  tools ausentes en la release npm; el cliente manda **lo que el server diga**.
- **Respuestas grandes.** REA tiene *Evidence IDs* y vistas paginadas;
  truncar y preferir `summary`/`inspect_analysis_view` antes que repetir la
  corrida (lo dice el propio skill).
- **Aprobación.** Muchas tools REA **ejecutan** el target o lanzan motores:
  `confirm: true` por defecto para servers MCP que ejecutan procesos.
- **Windows.** Expectativas: motores nativos limitados (sin Hopper, Ghidra P0);
  el resto del catálogo REA es estático y funciona.
- **Relación con skills.** MCP y skills son **complementarios**: el skill da el
  *procedimiento*; MCP da las *tools*. El plan [`SKILLS.md`](SKILLS.md) sigue
  siendo la base.
