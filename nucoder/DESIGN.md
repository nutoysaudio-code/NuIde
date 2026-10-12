# Diseño de NuCoder

## Objetivo

CLI independiente y agente de código para NuIde que:

1. es un **router de proveedores AI gratis primero** (por tarea + costo),
2. **carga sus propias API keys** en su propia config,
3. es un **coder** (herramientas + bucle agéntico),
4. **rota** de proveedor sin perder la conversación cuando se agota la cuota,
5. tiene la lógica de decisión en **SpASM** (AOT → C → FFI),
6. expone un **núcleo UI-agnóstico** (`src/lib.rs`); la UI de chat es un **panel
   nativo de NuIde** (Slint) y el binario `nucoder` es el **CLI**.

## División de responsabilidades

| Capa | Responsabilidad |
|---|---|
| **SpASM** (`spasm/*.spasm`) | Enrutamiento (tarea + gratis + costo) y rotación por `excluded` |
| **Rust** (`src/`) | Config, HTTP/streaming, herramientas, bucle del agente. Núcleo UI-agnóstico; el panel de chat vive en NuIde (`ui/ide_nucoder.slint` + `src/nucoder_ui.rs`) |
| **FFI** (`ffi/*.c`) | Puente estable entre el C generado y `src/ffi.rs` |

El costo se normaliza a entero (×100 USD/Mtok) y la capacidad a `power` (0..9)
para que SpASM compare enteros y no tenga que parsear JSON ni floats.

## Flujo

```
config propia  ~/.config/nucoder/config.json
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

## Filtro de "gratis" por proveedor

### Problema

El router consume dos cosas: el booleano `free` de cada modelo (**gratis
primero**) y el flag `free_only` del proveedor. Pero **"gratis" no es
uniforme**: cada endpoint lo señala distinto y algunos no lo señalan. Hoy
`discover` sólo mira el nombre (`:free`/`free`), así que con NVIDIA NIM —que no
etiqueta— no guarda nada; y `parse_own_model` marca gratis a **todo** modelo con
`cost_scaled == 0`, lo que con datos descubiertos sin precio vuelve gratis al
catálogo entero.

### Señales observadas

| Señal | Ejemplos | Detección |
|---|---|---|
| `pricing` en 0 | OpenRouter, Together, DeepInfra, Fireworks, Novita, Hyperbolic | hay precios y suman 0 |
| marcador en id/nombre | OpenRouter (`:free`), otros | `:free`, `(free)`, "free"/"gratis"/"trial" |
| **sin señal**; el catálogo es free tier | NVIDIA NIM, Groq, Cerebras, Gemini, Mistral, SambaNova | "todos gratis" (con cuota) |
| sin señal y mezcla free/pago | endpoints raros | lista explícita del usuario |

Evidencia (payload real):

- NVIDIA: `{"id":"01-ai/yi-large","object":"model","owned_by":"01-ai"}` → **sin precio ni flag**.
- OpenRouter: `{"id":"...:free","pricing":{"prompt":"0","completion":"0"}}` → **precio 0 + sufijo**.

### Modelo: `free_filter` por proveedor

Se agrega a cada proveedor de la config:

```json
{
  "id": "openrouter",
  "free_filter": "auto",
  "free_match": [":free", "(free)"],
  "free_only": true,
  "models": [ { "id": "...", "free": true } ]
}
```

- `free_filter` (string, opcional; default `auto`):
  `auto` | `all` | `none` | `name` | `pricing` | `list`.
- `free_match` (array de strings, opcional): marcadores/globs para `name` y
  `list`. Default: `:free`, `(free)`, `free`, `gratis`, `trial`.

Semántica:

- `all`: todo el catálogo es gratis (NVIDIA NIM, Groq, …).
- `none`: ninguno; nunca se auto-marca.
- `name`: gratis si id/nombre contiene algún `free_match`.
- `pricing`: gratis si el modelo trae precios y **todos** son 0.
- `list`: gratis sólo si id/nombre matchea algún glob de `free_match`
  (ej. `nvidia/*`, `*:free`).
- `auto`: resuelve por señales, en orden: `pricing` → `name` →
  (`free_only` ? `all` : `none`).

### Presets por `id` (cuando no hay `free_filter`)

| id | filtro |
|---|---|
| openrouter | `auto` |
| together, deepinfra, fireworks, novita, hyperbolic | `pricing` |
| groq, cerebras, nvidia, gemini, mistral, sambanova | `all` |
| desconocido | `auto` (→ `none` si no hay señal) |

El campo del usuario **pisa** el preset.

### Regla clave (sin señal)

Si el modelo no trae precio ni marcador y el proveedor **no** es `free_only` →
`free = false`. **Nunca** se usa `cost_scaled == 0` como proxy de gratis para
modelos descubiertos: `discover`/auto-descubrimiento persisten un `free`
**explícito** por modelo.

### Dónde se evalúa

En **Rust**, al descubrir (no en SpASM). El resultado queda como `free` por
modelo —lo que ya consume el router—, así que **SpASM no cambia**.

```
fn classify(filter, raw, id, name, provider_free_only) -> bool
  match filter
    All     -> true
    None    -> false
    Name    -> any(match(id, name, free_match))
    List    -> any(glob(id, name, free_match))
    Pricing -> let p = prices(raw);  p.is_some() && p == 0
    Auto    -> if let Some(p) = prices(raw) { p == 0 }
               else if match(id, name)      { true }
               else                         { provider_free_only }
```

### Cambios de código previstos

1. `client::ModelInfo` → guarda `raw: serde_json::Value` (hoy descarta el pricing).
2. `src/freefilter.rs` (nuevo): `FreeFilter`, `classify(...)`, `preset(id)`,
   `prices(raw)` (detecta `pricing.prompt/completion`, `pricing.input/output`, …).
3. `config::Provider` → `free_filter: FreeFilter`, `free_match: Vec<String>`
   (parseo y `provider_to_json`).
4. `discover` y `auto_discover` → clasifican con el filtro y escriben `free`
   explícito; `--save-free`/`--save-all` se redefinen sobre el filtro (el
   fallback actual "sin etiqueta + `free_only`" queda cubierto por `auto`).
5. CLI: `provider add --free-filter <modo> [--free-match <pat>]…`; `--providers`
   muestra el filtro de cada proveedor.
6. `nucoder refresh [id]` (posterior) para re-bajar y reclasificar sin perder
   overrides manuales (`--reset-free` para forzarlo).

### Overrides manuales

El `free` por modelo sigue editable a mano. Un `refresh` debe **preservar** los
`free` fijados manualmente salvo que se pida `--reset-free`.

### Pruebas (fixtures de payload)

- NVIDIA (sin precio) + `free_only` → todos `free = true`.
- OpenRouter `:free` y pago (precio > 0) → sólo el free.
- Proveedor sin señal y sin `free_only` → ninguno free.
- `free_filter` explícito (`all`/`none`) gana al preset.

> **Estado:** implementado. `free_filter`/`free_match` viven en
> `src/freefilter.rs` (modos + presets), `client::ModelInfo` guarda el `raw`,
> `discover`/auto-descubrimiento clasifican y persisten `free` explícito, y
> `provider add <preset>` resuelve base URL y filtro. Falta sólo `nucoder refresh`.

## Capacidades por modelo

Además de gratis/potencia/costo, cada modelo tiene **capacidades** (`src/caps.rs`):
`text`, `code`, `vision`, `image`, `audio`, `video`, `embedding`, `guard`.

- Se infieren del **payload** (`architecture.input_modalities`/
  `output_modalities`/`modality` de OpenRouter) y, si falta, por **heurística**
  de id/nombre (`whisper`→audio, `sdxl`→imagen, `coder`→código, `-vl`→visión…).
  Se pueden declarar a mano con `"caps": [...]`.
- Se persisten en la config (`"caps"` por modelo), porque el `raw` no se guarda.
- Se usan para **listar/filtrar** (`nucoder models --free --cap audio`) y para
  **restringir el router** de chat (`--cap` / `/cap`).
- Nota: el coder usa `POST /chat/completions`; sólo `text` es ruteable. Imagen,
  audio y video usan otros endpoints (`/images/generations`, `/audio/...`), así
  que por ahora sólo se listan/filtran (invocarlos es otra fase).

## Pendientes

- **`nucoder refresh`**: ✅ `nucoder refresh [id] [--reset-free]` re-baja el
  catálogo y reclasifica **conservando** los `free` fijados a mano (con
  `--reset-free` los recalcula desde el filtro). `config::merge_discovered`.
- **Invocar endpoints no-chat** (imagen/audio/video) desde el agente: ✅
  `image_generate` (`/images/generations`, guarda en `generated/`) y
  `audio_transcribe` (`/audio/transcriptions`); eligen el modelo por capacidad
  (`image`/`audio`) y respetan el permiso de internet. Video/embedding, pendiente.
- **Completado semántico**: análisis de AST con `spasm-asm-gen.py` (hoy el módulo
  `codeassist.spasm` da keywords).
- **Persistencia**: historial de conversaciones por proyecto.
- **Selección manual** de proveedor/modelo: ✅ en el panel de NuIde (selector de
  modelo + tarea; el CLI lo persiste en `default_model`).
- **Config de proveedores desde la TUI** (hoy por CLI: `provider add`): el CRUD
  vive en NuIde (Preferencias → Proveedores).
- **Más formas de auth** (Anthropic, cabeceras propias) si se suman proveedores.
