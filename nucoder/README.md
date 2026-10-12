# NuCoder

Agente de código para la terminal con **router de proveedores
AI gratis primero**. La **decisión de enrutamiento vive en SpASM** (dialecto
compilado AOT → C → FFI). El **núcleo** (router, config, cliente HTTP, agente y
herramientas) es una **librería UI-agnóstica** (`src/lib.rs`); la interfaz de
chat es un **panel nativo de NuIde** (Slint) y el binario `nucoder` queda como
**CLI** (`ask`, `provider`, `models`, `discover`, `route`, `config`).

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
| Bucle agéntico con herramientas (read/write/edit/delete/move/list/glob/grep/bash) | ✅ real |
| `run_in_terminal`: el agente escribe comandos en las **terminales reales de NuIde** (servidores, watchers) | ✅ real (host IDE) |
| Rotación por cuota/autenticación/servidor y modelo no disponible (404) | ✅ real |
| Internet (`web_fetch`/`web_search` vía curl) | ✅ real |
| KB local (`read_docs`) + *primer* de SpASM en el prompt de sistema | ✅ real |
| Sandbox de archivos (confina al proyecto) | ✅ real |
| Confirmación antes de borrar/mover/bash/terminal (**switch auto-aprobar**) | ✅ real |
| Descubrimiento de modelos (`GET /models`), filtro de gratis y auto-descubrimiento | ✅ real |
| Presets de proveedores (`provider add <id>`) + `free_filter` por proveedor | ✅ real |
| Capacidades por modelo (text/code/vision/image/audio/video/embedding/guard) | ✅ real |
| Listado/filtro por capacidad (`models`, `--cap`, `/cap`) | ✅ real |
| Imágenes en el chat (visión) con `/img` y `--img` | ✅ real |
| UI de chat nativa en NuIde (Slint): streaming, historial, tarea, switch de auto-aprobar | ✅ real |
| Chat con **Markdown** + **resaltado de sintaxis** de los bloques de código | ✅ real (núcleo `markdown`) |
| Paleta alineada con el tema de NuIde (`ui/ide_theme.slint`) | ✅ real |
| Integración con NuIde (paleta → *Lanzar NuCoder*) | ✅ panel nativo |
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

La **interfaz de chat** es el **panel nativo de NuIde** (menú *Agente* →
*Lanzar NuCoder*, o la paleta). El binario `nucoder` es el **CLI**:

```powershell
nucoder ask "resumí el repo"  # one-shot: imprime la respuesta
nucoder --img foto.png --ask "¿qué se ve?"   # one-shot con imagen (visión)
nucoder --route refactor      # qué proveedor/modelo elegiría
nucoder --providers           # lista proveedores y modelos
nucoder config                # muestra la config (auto-aprobar, sandbox, gratis…)
nucoder config auto-approve on   # no pedir confirmación (borrar/mover/bash/terminal)
nucoder provider presets      # presets conocidos (openrouter, nvidia, groq, …)
nucoder provider add openrouter --api-key sk-or-...   # alta por preset
nucoder refresh openrouter    # re-baja y reclasifica (conserva los `free` manuales)
nucoder refresh --reset-free  # idem, recalculando los `free` desde el filtro
nucoder models --free --cap image                     # gratis por capacidad
nucoder --free                # fuerza sólo modelos gratuitos
nucoder --cap code            # restringe el router a modelos de código
nucoder --model <id> --ask "…"   # fuerza un modelo (sin router ni rotación)
nucoder --no-sandbox          # permite rutas fuera del proyecto
nucoder --no-confirm          # no pedir confirmación (equivale a auto-approve on)
```

Sin argumentos, `nucoder` imprime la ayuda (el chat vive en NuIde).

### Herramientas

`read_file`, `write_file`, `edit_file`, `delete_path`, `move_file`, `list_dir`,
`glob`, `grep`, `bash` (shell), `run_in_terminal`, `web_fetch`, `web_search`,
`read_docs` (KB), `submit_plan` (plan), `ftp_*` (`ftp_open`/`ftp_list`/
`ftp_upload`/`ftp_download`/`ftp_delete`/`ftp_mkdir`), `image_generate` y
`audio_transcribe`.

- **Permisos** (pestaña **Permisos** del panel, o config): **Internet**
  (`web_fetch`/`web_search`), **leer fuera del proyecto**, **terminales de la
  IDE** (`run_in_terminal`), **cliente FTP** (`ftp_*`), **sandbox de
  escrituras**, **confirmación** y **autopiloto**. Se persisten en
  `"permissions": { web, read_outside, terminal, ftp }` + `sandbox`, `confirm`,
  `"autopilot"`.
- **Cliente FTP** (`ftp_*`): el agente opera el **cliente FTP de la IDE**
  (`ftp_open` lo abre/navega; `ftp_list`/`ftp_upload`/`ftp_download`/`ftp_delete`/
  `ftp_mkdir` hacen el resto). Sólo funciona con el cliente FTP abierto en la IDE.
- **Plan + autopiloto**: para tareas de varios pasos el agente llama primero a
  `submit_plan` (objetivo + pasos) y el panel muestra el plan con
  *Aprobar*/*Rechazar*. Aprobado el plan, si el **autopiloto** está prendido el
  agente ejecuta todos los pasos **sin pedir confirmación** hasta terminar; si
  no, sigue el plan pero confirmando cada acción sensible.

- **Sandbox** (ON por defecto): las tools de archivos quedan confinadas al
  proyecto (`--cwd`); la KB es de lectura permitida. `bash` **no** se confina
  (limitación conocida). Se desactiva con `--no-sandbox` o `"sandbox": false`.
- **Confirmación** (ON por defecto): antes de `delete_path`, `move_file`, `bash`
  o `run_in_terminal`, el **panel de NuIde** pide aprobación (botones
  *Aprobar*/*Rechazar*). El **switch auto-aprobar** (cabecera del panel, o
  `nucoder config auto-approve on`) lo saltea por completo y queda persistido.
  En one-shot (`ask`) no hay interacción y la confirmación queda apagada.
- **`run_in_terminal`** (sólo dentro de NuIde): en vez de capturar la salida,
  el agente **escribe el comando en una terminal real de la IDE**, que queda
  visible y sigue corriendo. Es lo indicado para servidores de desarrollo y
  watchers (`npm run dev`, `python -m http.server`, `cargo run`). Con `name`
  puede elegir la terminal (coincide con el nombre de la sesión, p. ej. `build`);
  sin `name` va a la terminal local. Para comandos puntuales cuya salida
  necesita, usa `bash`.
- **Internet**: `web_fetch` baja una URL (texto limpio si es HTML) y `web_search`
  consulta DuckDuckGo. Usan `curl.exe` (`NUCODER_CURL` lo cambia).

### Conocimiento local (KB)

El prompt de sistema incluye un *primer* de SpASM y la lista de documentos; el
modelo los lee con `read_docs`. Fuentes:

1. `knowledge` en la config (rutas a archivos o carpetas);
2. `NUCODER_KNOWLEDGE` (rutas separadas por `;`);
3. autodetección: `F:\SpAMS-Oficial\SPASM_QUICKREF.md` y
   `...\skills\spasm-architect`, más `SPASM_HOME` y la raíz del proyecto si
   tienen esa estructura.

En el **panel de NuIde**: `Enter` (o *Enviar*) manda; **Detener** cancela el turno;
los switches **gratis** y **auto-aprobar** alternan sólo-gratis y las
confirmaciones; **nueva** reinicia el chat y la sesión. En la cabecera, el
**modelo** abre un selector (auto + modelos curados, gratis primero) que se
persiste en `default_model`. En la barra: la **tarea** cicla
(`complete → refactor → …`), **imagen** adjunta una imagen (modelos con visión;
el chip `N img ✕` la quita) y **ctx** cicla el tope de contexto. El chip
**permisos** abre la pestaña de permisos (internet, leer fuera del proyecto,
terminales, ftp, sandbox, confirmación, autopiloto) y **modelos** abre el
listado de modelos (filtro por nombre/especialidad, especialidad —coder, imagen,
audio…—, gratis/poder, y **probar/ping** por modelo y «probar todos»). Los
**atajos
globales** (paleta, buscar archivos, buscar en el proyecto, ayuda) funcionan
aunque el foco esté en el input del chat. La conversación se guarda/carga sola
por proyecto.

> La TUI anterior (`ratatui`) se reemplazó por el panel nativo, que ya cubre
> tarea, modelo, contexto, imágenes, permisos, autopiloto, auto-aprobar,
> `run_in_terminal` y atajos.

El modelo forzado se guarda en `default_model` de la config y se usa al arrancar
(se puede pisar con `--model <id>`).

### Imágenes (visión)

Si adjuntás imágenes, el turno se rutea **sólo a modelos con visión** (el header
muestra `· N img`). El archivo se convierte a un *data URL* (`data:image/...;base64,`)
y viaja en el mensaje `user` como `image_url` (formato OpenAI). Formatos:
png/jpg/jpeg/webp/gif, hasta 10 MB. En one-shot:

```powershell
nucoder --img grafico.png --ask "explicá el gráfico"
```

Si no hay ningún modelo con visión entre los candidatos, avisa al adjuntar y el
turno no encuentra modelo.

### Memoria, contexto y caché

- **Memoria**: la conversación se mantiene en RAM entre turnos. `/clear` (o
  `/reset`) la reinician. Las **imágenes** se guardan sólo durante su turno y
  después se liberan (no se reenvían).
- **Contexto**: si el historial supera el tope aproximado, se **descartan los
  turnos más viejos** (sin cortar pares assistant/tool) para no exceder el context
  window. El tope (default ~48 000 caracteres) se ajusta con
  `"context_chars"` en la config, `--context-chars <n>` o `NUCODER_CONTEXT_CHARS`
  (en ese orden de prioridad); en la TUI, `/context` lo muestra y `/context <n>`
  lo cambia (y lo persiste).
- **Persistencia por proyecto**: el historial se guarda en
  `~/.config/nucoder/sessions/` (clave por `--cwd`) y se **recarga al arrancar**;
  `/save` y `/load` son manuales, y al salir se guarda solo. `/clear` borra la
  sesión guardada.
- **Caché de render**: la TUI memoiza el Markdown por texto+ancho y sólo
  materializa la ventana visible (no re-parsea todo en cada frame).
- **Prompt caching**: `--prompt-cache` (o `"prompt_cache": true`) marca el
  `system` como bloque cacheable; lo aprovechan proveedores que lo soportan
  (Anthropic/OpenRouter) y los demás lo ignoran. La respuesta del asistente se **renderiza como Markdown**
(títulos, listas, tablas, enlaces, negrita/itálica) y los **bloques de código se
resaltan por sintaxis** según el lenguaje de la cerca.

Los colores (`src/theme.rs`) siguen la paleta de NuIde (`ui/ide_theme.slint`):
fondo panel, marfil para texto, **mint** para acentos/modelo, **amber** para
herramientas y el mismo esquema semántico del editor (keyword/string/number/
comment). Se emiten en **truecolor**; el emulador de la IDE y las terminales
modernas lo soportan.

### Proveedores, gratis y capacidades

**Presets.** `nucoder provider presets` lista los conocidos; dar de alta es una
línea (la base URL y el filtro de gratis salen del preset):

```powershell
nucoder provider add openrouter --api-key sk-or-...
nucoder provider add nvidia     --api-key nvapi-...
nucoder provider add groq       --api-key gsk_...
```

Un preset arranca como `free_only` (catálogo free-tier) salvo `--no-free-only`.

**Filtro de gratis por proveedor.** Cada endpoint marca los gratuitos distinto;
`free_filter` decide cómo detectarlos (y el resultado se guarda como `free`
explícito por modelo):

| modo | qué mira |
|---|---|
| `auto` (default) | precios → nombre (`:free`…) → `free_only` |
| `all` | todo el catálogo es gratis (NVIDIA NIM, Groq, Cerebras…) |
| `none` | ninguno |
| `name` | marcador en id/nombre |
| `pricing` | precio 0 (`pricing.prompt/completion`, `input/output`) |
| `list` | patrones glob en `free_match` (p. ej. `nvidia/*`) |

```powershell
nucoder discover openrouter            # lista gratis/pagos + capacidades
nucoder discover openrouter --save-free
nucoder discover nvidia --save-all     # o --save-free (filtro `all` ⇒ todo)
```

Un proveedor `free_only` **sin modelos** se **autodescubre al arrancar** (TUI,
`ask`, `route`, `--providers`).

**Capacidades.** Se infieren del payload (`architecture`/modalidades de
OpenRouter) y con heurística por nombre: `text`, `code`, `vision`, `image`,
`audio`, `video`, `embedding`, `guard`. Listá/filtrá con `nucoder models` o
restringí el router con `--cap` / `/cap`:

```powershell
nucoder models --free                      # todos los gratuitos
nucoder models --free --cap audio          # gratis con audio
nucoder models --provider openrouter --cap image
nucoder --cap code ask "refactorizá esto"  # router sólo con modelos de código
```

> El coder usa `POST /chat/completions`: sólo los modelos con `text` son
> ruteables. `image`/`audio`/`video`/`embedding` sirven para **listar/filtrar**
> el catálogo (sus endpoints son otros).

La lista de proveedores se guarda en la **config propia** del CLI:
`~/.config/nucoder/config.json` (override con `NUCODER_CONFIG`). Formato:

```json
{
  "free_only": false,
  "default_model": null,
  "sandbox": true,
  "confirm": true,
  "auto_approve": false,
  "permissions": { "web": true, "read_outside": false, "terminal": true },
  "autopilot": false,
  "knowledge": ["F:/SpAMS-Oficial/skills/spasm-architect"],
  "prompt_cache": false,
  "context_chars": 48000,
  "providers": [
    {
      "id": "openrouter",
      "name": "OpenRouter",
      "base_url": "https://openrouter.ai/api/v1",
      "api_key": "sk-or-...",
      "free_only": true,
      "free_filter": "auto",
      "free_match": [],
      "models": [
        {
          "id": "deepseek/deepseek-r1:free",
          "name": "DeepSeek R1 (free)",
          "free": true,
          "power": 6,
          "caps": ["text", "code"]
        }
      ]
    }
  ]
}
```

Si existe `provider` en `api_key`, se prioriza la key; si querés una variable de
entorno, usá `"env": ["OPENROUTER_API_KEY"]`. Los modelos también
se pueden declarar como `"models": ["id-del-modelo"]`.

> **Las API keys nunca van al repo.** Viven en la config del usuario
> (`~/.config/nucoder/config.json`). Hay un test guardián
> (`providers::el_repo_no_tiene_keys_versionadas`) que falla si una key con forma
> de key queda versionada. Ojo: un hash (MD5 + sal) **no** sirve para keys —hay
> que poder usarlas—; si se quiere cifrar en reposo, es cifrado reversible
> (DPAPI / Credential Manager), no hashing.

## Arquitectura

```
spasm/*.spasm ── spasmc.py --target c ──▶ spasm/generated/*.c  (commiteado)
ffi/*_ffi.c   incluye el generado y exporta los simbolos nucoder_*
build.rs (cc) ──▶ staticlib enlazada a nucoder.exe y a nuide.exe
src/lib.rs    nucleo UI-agnostico: lo comparten el CLI y el panel de NuIde
src/ffi.rs    extern "C" + wrappers (mutex: el C usa buffers estaticos)
src/config.rs config propia (API keys, modelos, capacidades, auto_approve)
src/client.rs cliente HTTP (curl) con streaming SSE y clasificacion de errores
src/caps.rs   capacidades del modelo (text/code/vision/image/audio/…)
src/freefilter.rs filtro de gratis por proveedor + presets de alta
src/knowledge.rs indice de la KB local (read_docs) y primer de SpASM
src/agent.rs  bucle del agente + rotacion + confirmacion de acciones sensibles
src/tools.rs  herramientas (read/write/edit/delete/move/web/read_docs/run_in_terminal)
src/markdown.rs  segmenta Markdown en prosa + bloques de codigo tokenizados
src/main.rs   CLI (one-shot, proveedores, discover, models, config)
```

El **panel de chat** vive en NuIde: `ui/ide_nucoder.slint` (Slint) +
`nuide/src/nucoder_ui.rs` (estado, canal de eventos, modelo de mensajes). El
mismo núcleo corre el agente en un hilo; NuIde es el **host de terminales** y
atiende `run_in_terminal`.

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
