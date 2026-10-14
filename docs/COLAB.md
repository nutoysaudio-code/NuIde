# Correr un modelo "gratis" (Colab, o un server propio) y usarlo desde NuCoder

NuCoder habla con **cualquier endpoint compatible-OpenAI**: no necesita que el
modelo sea un preset. Si levantás un modelo en Google Colab (GPU T4 gratis) y lo
exponés por una URL, alcanza con dar de alta un **proveedor** (`baseURL` +
`apiKey`) y el router lo usa como a cualquier otro.

> Antes de armar Colab: probá los **presets gratuitos** que ya trae NuCoder
> (`openrouter` con los `:free`, `gemini` con el tier gratis de Google AI Studio
> —tiene endpoint compatible-OpenAI—, `groq`, `cerebras`, `nvidia`). Muchas veces
> es más simple y no depende de una VM que se apaga.

---

## 1. Levantar un server compatible-OpenAI en Colab

En una celda (runtime GPU):

**Opción A — vLLM (recomendado, ya es OpenAI-compatible):**

```bash
!pip -q install vllm
!python -m vllm.entrypoints.openai.api_server \
    --model Qwen/Qwen2.5-Coder-7B-Instruct \
    --api-key mi-clave-secreta \
    --host 0.0.0.0 --port 8000
```

**Opción B — llama.cpp (`llama-server`, también compatible):**

```bash
!CMAKE_ARGS="-DGGML_CUDA=on" pip -q install llama-cpp-python[server]
!python -m llama_cpp.server --model modelo.gguf --host 0.0.0.0 --port 8000
```

**Opción C — Ollama:** `ollama serve` con `OLLAMA_HOST=0.0.0.0:8000` (su API
`/v1` es compatible).

El endpoint queda en `http://localhost:8000/v1` (dentro de Colab).

## 2. Exponerlo a Internet (Colab no abre puertos)

Los túneles funcionan; el más simple es **Cloudflare** (no pide cuenta):

```bash
!wget -q https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -O cloudflared
!chmod +x cloudflared
!./cloudflared tunnel --url http://localhost:8000
```

La salida imprime una URL tipo `https://algo-aleatorio.trycloudflare.com`.

- Con **vLLM**: el `baseURL` es `https://algo-aleatorio.trycloudflare.com/v1`.
- Poné `--api-key` en el server y usá la misma clave en NuCoder (el túnel es
  **público**: sin clave, cualquiera puede usar tu GPU).

## 3. Darlo de alta en NuCoder

**Desde la IDE:** `Ctrl+,` → **Proveedores** → *Crear*.
- **ID**: `colab`
- **baseURL**: `https://algo-aleatorio.trycloudflare.com/v1`
- **apiKey**: `mi-clave-secreta`
- **Modelos**: JSON con el modelo servido, p. ej.
  ```json
  { "qwen2.5-coder-7b": { "name": "Qwen2.5 Coder 7B (Colab)", "free": true, "power": 3, "caps": ["text"] } }
  ```

**Por CLI:**
```
nucoder provider add colab --name Colab --base-url https://algo-aleatorio.trycloudflare.com/v1 --api-key mi-clave-secreta
nucoder discover colab      # intenta listar modelos del endpoint
nucoder models
```

Después, en el panel de NuCoder, elegí ese modelo (o dejá el router; si marcaste
`free` entra en el pool "solo gratis").

---

## 4. Límites honestos (Colab)

- La VM de Colab **se corta**: por inactividad, por tiempo (~12 h) y al cerrar la
  pestaña. Cuando se corta, **la URL del túnel cambia** → hay que actualizar el
  `baseURL` del proveedor.
- Colab puede **bloquear o limitar** túneles/uso de red; no hay garantía.
- El endpoint es **público**: usá `--api-key` y no lo dejes abierto.
- El modelo queda **en la memoria de la VM**: no persiste entre sesiones (hay que
  reinstalar/recargar). Para algo estable, usá un preset gratuito o un server
  propio (misma receta, sin Colab).

> En resumen: Colab sirve para **probar** un modelo grande gratis de vez en
> cuando, no como backend fijo. Para el día a día, los presets gratuitos.
