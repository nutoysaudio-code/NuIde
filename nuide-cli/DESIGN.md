# Diseño de Nuide-CLI

## Objetivo

CLI independiente y agente de código para NuIde que:

1. es un **router de proveedores AI gratis primero** (por tarea + costo),
2. **carga sus propias API keys** (y además puede leer la config de OpenCode),
3. es un **coder** estilo OpenCode (herramientas + bucle agéntico),
4. **rota** de proveedor sin perder la conversación cuando se agota la cuota,
5. tiene la lógica de decisión en **SpASM** (AOT → C → FFI),
6. usa una **TUI** (`ratatui` + `crossterm`).

## División de responsabilidades

| Capa | Responsabilidad |
|---|---|
| **SpASM** (`spasm/*.spasm`) | Enrutamiento (tarea + gratis + costo) y rotación por `excluded` |
| **Rust** (`src/`) | Config, HTTP/streaming, herramientas, bucle del agente, TUI |
| **FFI** (`ffi/*.c`) | Puente estable entre el C generado y `src/ffi.rs` |

El costo se normaliza a entero (×100 USD/Mtok) y la capacidad a `power` (0..9)
para que SpASM compare enteros y no tenga que parsear JSON ni floats.

## Flujo

```
config propia  ~/.config/nuide-cli/config.json      ┐
config OpenCode ~/.config/opencode/opencode.json(c) ┴─▶ Config (union)
        │
        ▼
   Vec<Candidate {free, power, cost, ...}>
        │
        │  config.route(task, excluded) → ffi.router_pick(task, spec, excluded)
        ▼
   spasm/router.spasm (AOT → C):  elige el indice (gratis primero)
        │
        ▼
   client.chat_stream(base_url, api_key, model, messages, tools)   [curl, SSE]
        │  ├─ delta de texto ──────────────▶ TUI (streaming)
        │  └─ tool_calls ─▶ tools.execute ─▶ mensaje "tool" ─┐
        │                                                   │ (bucle)
        │◀──────────────────────────────────────────────────┘
        └─ Quota/Auth/Server → excluded.push(idx) → reintenta con el router
```

## Reglas del puente (las impone el backend C)

1. Las `func` de SpASM sólo retornan `int/float`; los strings salen por
   **globales del módulo** (ej. `codeassist_out`).
2. Cada builtin de string usa un **buffer estático**: no se anidan `str_slice`.
   Por eso el router parsea la lista en **una sola pasada** y sólo compara enteros.
3. **Thread-safety**: el C generado no tiene; `src/ffi.rs` serializa con un `Mutex`.
4. El `main` del programa AOT se renombra con un macro en cada wrapper.

## Cliente HTTP (sin crates de red)

Se usa `curl.exe` como proceso hijo, consistente con el resto del ecosistema:

- la API key viaja por un archivo de configuración de curl (`-K`), **no** en la
  línea de comandos, para no exponerla en la lista de procesos;
- `--no-buffer` para streaming; el código HTTP se lee del marcador `-w`
  (`@@HTTP<code>@@`) al final del cuerpo;
- errores: `429/402/quota` → `Quota`, `401/403` → `Auth`, `5xx` → `Server`; los
  tres disparan rotación. `4xx` restantes → `Http` (fatal).

## Herramientas del agente

`read_file`, `write_file`, `edit_file`, `list_dir`, `glob`, `grep`, `bash`.
Implementadas con `std` (sin crates): recorrido recursivo que saltea generados
(`target`, `node_modules`, `.git`, …), glob propio (`*`, `?`, `**`) y salidas
acotadas (32 KB).

## Pendientes

- **Completado semántico**: análisis de AST con `spasm-asm-gen.py` (hoy el módulo
  `codeassist.spasm` da keywords).
- **Persistencia**: historial de conversaciones por proyecto.
- **Selección manual** de proveedor/modelo desde la TUI (`/model`).
- **Config de proveedores desde la TUI** (hoy por CLI: `provider add`).
- **Más formas de auth** (Anthropic, cabeceras propias) si se suman proveedores.
