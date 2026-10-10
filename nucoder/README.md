# NuCoder

Agente de código para la terminal con **router de proveedores
AI gratis primero**. La **decisión de enrutamiento vive en SpASM** (dialecto
compilado AOT → C → FFI) y la interfaz es una **TUI** (`ratatui` + `crossterm`).

- **Router propio**: cargás las **API keys** de los modelos gratuitos (OpenRouter,
  Groq, Cerebras, Gemini vía su endpoint OpenAI-compatible, etc.) en la config del
  propio CLI.
- **Coder**: bucle agéntico con herramientas para leer, escribir, editar y buscar
  archivos, y ejecutar comandos; streaming en vivo.
- **Rotación**: si un proveedor se queda sin cuota (429/402), la key es inválida
  (401/403) o da 5xx, **rota al siguiente candidato sin perder la conversación**.

## Estado

| Componente | Estado |
|---|---|
| Router de proveedores/modelos (gratis primero + rotación) | ✅ real (SpASM) |
| Config **propia** con API keys | ✅ real |
| Cliente HTTP compatible-OpenAI con **streaming SSE** (`curl`) | ✅ real |
| Bucle agéntico con herramientas (read/write/edit/glob/grep/bash) | ✅ real |
| Rotación por cuota/autenticación/servidor | ✅ real |
| Descubrimiento de modelos (`GET /models`) y filtro de gratis | ✅ real |
| TUI de chat (streaming, historial, tarea, modo gratis) | ✅ real |
| Integración con NuIde (paleta → *Lanzar NuCoder*) | ✅ |
| Completado semántico por AST (SpASM) | ⏳ pendiente |

## Cómo funciona el router

La decisión vive en `spasm/router.spasm`. Cada candidato se codifica como
`free,power,cost` y Rust pasa la lista (más los índices ya agotados):

```
"1,7,0;0,9,250;1,2,0"   +   excluded = "0"
```

Reglas (mejor → peor):

1. **gratuitos** antes que pagos;
2. tareas de **calidad** (`refactor`/`debug`/`explain`/`architect`/`test`) →
   mayor `power`; tareas **rápidas** (`complete`/`chat`) → menor costo;
3. desempate y, finalmente, orden de aparición.

Cuando un proveedor falla por cuota/autenticación, su índice se agrega a
`excluded` y se vuelve a llamar al router: la conversación (una lista de mensajes)
se **reenvía tal cual** al siguiente candidato.

## Uso

```powershell
nucoder                       # TUI interactiva (chat + herramientas)
nucoder ask "resumí el repo"  # one-shot: imprime la respuesta
nucoder --route refactor      # qué proveedor/modelo elegiría
nucoder --providers           # lista proveedores y modelos
nucoder --free                # fuerza sólo modelos gratuitos
```

En la TUI: `Enter` envía; `/refactor`, `/debug`, `/explain`, `/test` cambian la
tarea; `/free` alterna sólo-gratis; `/clear`; `/quit`. `Esc` cancela el turno en
curso y `Ctrl+C` sale.

### Cargar una API key gratuita

```powershell
# OpenRouter (modelos :free)
nucoder provider add --id openrouter --name OpenRouter `
    --base-url https://openrouter.ai/api/v1 --api-key sk-or-... --free-only

# Descubrir modelos del endpoint y quedarse con los gratuitos
nucoder discover openrouter --save-free

# Gemini (endpoint OpenAI-compatible)
nucoder provider add --id gemini --name Gemini `
    --base-url https://generativelanguage.googleapis.com/v1beta/openai --api-key AIza...
```

La lista de proveedores se guarda en la **config propia** del CLI:
`~/.config/nucoder/config.json` (override con `NUCODER_CONFIG`). Formato:

```json
{
  "free_only": false,
  "providers": [
    {
      "id": "openrouter",
      "name": "OpenRouter",
      "base_url": "https://openrouter.ai/api/v1",
      "api_key": "sk-or-...",
      "free_only": true,
      "models": [
        { "id": "deepseek/deepseek-r1:free", "name": "DeepSeek R1 (free)", "free": true, "power": 6 }
      ]
    }
  ]
}
```

Si existe `provider` en `api_key`, se prioriza la key; si querés una variable de
entorno, usá `"env": ["OPENROUTER_API_KEY"]`. Los modelos también
se pueden declarar como `"models": ["id-del-modelo"]`.

## Arquitectura

```
spasm/*.spasm ── spasmc.py --target c ──▶ spasm/generated/*.c  (commiteado)
ffi/*_ffi.c   incluye el generado y exporta los simbolos nuide_*
build.rs (cc) ──▶ staticlib enlazada a nucoder.exe
src/ffi.rs    extern "C" + wrappers (mutex: el C usa buffers estaticos)
src/config.rs config propia (API keys y modelos); candidatos
src/client.rs cliente HTTP (curl) con streaming SSE y clasificacion de errores
src/agent.rs  bucle del agente + rotacion de proveedores
src/tools.rs  herramientas (read/write/edit/list_dir/glob/grep/bash)
src/tui.rs    chat ratatui
src/main.rs   CLI (TUI, one-shot, proveedores, discover)
```

| Modulo SpASM | Exporta |
|---|---|
| `spasm/router.spasm` | `nucoder_router_pick`, `nucoder_router_pick_ex`, `nucoder_router_is_power_task` |
| `spasm/codeassist.spasm` | `nucoder_codeassist_keywords` (+ global `codeassist_out`) |

Regenerar el C: `powershell -ExecutionPolicy Bypass -File tools/regen_spasm.ps1`
(requiere Python + `spasmc.py`; rutas con `NUIDE_PYTHON` / `NUIDE_SPASMC`).

## Compilar

```sh
cargo build --release --bin nucoder
# binario: target/release/nucoder.exe
```

Un clone limpio compila sin Python (el C va commiteado); sólo necesita un
compilador de C (el de MSVC que ya usa Rust en Windows) y `curl.exe` (viene con
Windows) para las peticiones.
