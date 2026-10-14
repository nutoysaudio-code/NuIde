# Alojar un modelo "gratis" (Kaggle / Colab / server propio) y usarlo desde NuCoder

NuCoder habla con **cualquier endpoint compatible-OpenAI**: no hace falta que el
modelo sea un preset. Si levantás un modelo en una GPU gratis (Kaggle o Colab) y
lo exponés por una URL, alcanza con dar de alta un **proveedor** (`baseURL` +
`apiKey`) y el router lo usa como a cualquier otro.

> **Antes de armar la VM**: para *codear todos los días* suele convenir un preset
> gratuito (persistente, sin túnel): `openrouter` (los `:free`), `gemini` (Google
> AI Studio, tier gratis, endpoint compatible-OpenAI), `groq`, `cerebras`,
> `nvidia`. Kaggle/Colab sirve para **probar** un modelo grande que no correrías
> de otra forma (p. ej. Qwen2.5-Coder-32B cuantizado), no como backend fijo.

---

## Kaggle (recomendado para GPU gratis)

- GPU gratis: **~30 h/semana** (T4 ×2 o P100). Sesión máxima **12 h**. Disco ~20 GB.
- Para usar Internet hay que **verificar el teléfono** en la cuenta y activar
  **Internet** en las opciones del notebook.
- Cómo: en una celda,

```bash
!pip -q install vllm
!python -m vllm.entrypoints.openai.api_server \
    --model Qwen/Qwen2.5-Coder-7B-Instruct \
    --api-key mi-clave-secreta --host 0.0.0.0 --port 8000 &

# Exponer con Cloudflare (no pide cuenta):
!wget -q https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -O cloudflared
!chmod +x cloudflared
!./cloudflared tunnel --url http://localhost:8000
```

La salida imprime `https://algo-aleatorio.trycloudflare.com`. El `baseURL` es esa
URL + `/v1`.

- **Persistencia**: guardá el notebook (**Save version**); al re-abrir, el entorno
  se recrea y **la URL del túnel cambia** → hay que actualizar el `baseURL` del
  proveedor. Los *Commits* de Kaggle reinician el entorno.
- Kaggle también sirve para **subir/bajar pesos** (Models/Datasets), útil si el
  modelo es grande.

## Colab (alternativa)

Igual que Kaggle (T4 gratis). Colab corta por inactividad (~90 min) y ~12 h,
y bloquea túneles a veces. La receta es idéntica (server + `cloudflared`).

## Server propio (lo estable)

La misma receta en cualquier máquina con GPU (o incluso CPU con llama.cpp): el
endpoint no cambia, no se corta y no depende de un túnel de terceros.

---

## Levantar el server (mismo en las tres)

- **vLLM** (ya es OpenAI-compatible): `python -m vllm.entrypoints.openai.api_server --model <hf-model> --api-key <k> --host 0.0.0.0 --port 8000`
- **llama.cpp**: `python -m llama_cpp.server --model modelo.gguf --host 0.0.0.0 --port 8000`
- **Ollama**: `ollama serve` con `OLLAMA_HOST=0.0.0.0:8000` (su `/v1` es compatible)

El endpoint queda en `http://localhost:8000/v1`.

---

## Darlo de alta en NuCoder

**Por CLI** (sirve para cualquier URL, con o sin preset):

```bash
nucoder provider add kaggle \
    --name "Kaggle (Qwen Coder)" \
    --base-url https://algo-aleatorio.trycloudflare.com/v1 \
    --api-key mi-clave-secreta \
    --free-only \
    --model qwen2.5-coder-7b

nucoder discover kaggle          # intenta listar los modelos del endpoint
nucoder models --free            # verifica que aparezca
```

**Desde la IDE**: `Ctrl+,` → **Proveedores** → *Crear* (ID `kaggle`, baseURL, apiKey,
y los modelos como JSON). Después, en el panel de NuCoder, elegí el modelo o dejá
el router (con `free` marcado entra en el pool "solo gratis").

> Nota: al dar de alta una **id nueva** con `--base-url`, poné `--free-only` si
> querés que el router lo trate como gratuito (los presets lo asumen solos).

---

## Límites honestos

- Kaggle/Colab **cortan** la sesión (inactividad, tope ~12 h, cerrar pestaña) y la
  **URL del túnel cambia** en cada corrida → hay que re-apuntar el `baseURL`.
- El túnel es **público**: usá `--api-key` en el server y la misma en NuCoder.
- El modelo vive en la **memoria de la VM**: no persiste entre sesiones.
- Las GPUs gratis tienen poca VRAM (T4 16 GB): hasta ~7B–13B **cuantizado**.
- Para el día a día de *codear gratis*, los **presets** gratuitos son más
  prácticos que una VM con túnel.
