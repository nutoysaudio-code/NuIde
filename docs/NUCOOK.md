# NuCook — destilar una habilidad, sin ruido humano

Documento de trabajo. La idea: **un skill es una habilidad**. Si está **bien
enseñada**, eso es todo lo que se necesita. NuCook no inventa un idioma ni
comprime: **quita el ruido y no deja huecos**.

> Relacionado: [`SKILLS.md`](SKILLS.md) (descubrimiento de skills) y
> [`MCP.md`](MCP.md) (herramientas). NuCook opera sobre lo que `SKILLS.md`
> descubre.

---

## 1. El criterio (lo importante)

"Sin pérdida" **no** es guardar el texto crudo —eso *es* conservar el ruido—:
es que **la habilidad siga siendo enseñable**. Y el ruido humano tiene dos caras:

1. **De más**: relleno, cortesía, repetición, meta-prosa, licencias, adornos.
2. **De menos**: **supuestos** que el humano comparte y no dice. Un skill "bien
   enseñado" para una IA es el que **no da por sabido nada necesario**.

Una **lengua común para IA** no se inventa: se **destila** de lo que ya
compartimos (lenguaje natural + unas pocas relaciones cognitivas). Cuanto menos
idioma inventes, más universal es. Las tripletas quedan donde son exactas
(`condición -> acción`), no como formato para todo.

---

## 2. La forma: una habilidad bien enseñada

Once relaciones, una por línea, con etiquetas cortas en la superficie que el
modelo ya conoce:

| etiqueta | parte | pregunta |
|---|---|---|
| `when` | cuándo aplica | ¿cuándo se usa? |
| `not` | cuándo **no** aplica | anti-trigger |
| `want` | qué logra | objetivo (1 línea) |
| `have` | qué necesita | entradas/precondiciones |
| `do` | qué hace | pasos/transformaciones |
| `keep` | qué conserva | invariantes (no romper) |
| `map` | ruteo | condición → acción/herramienta |
| `prove` | cómo se prueba | validación/evidencia |
| `eg` | ejemplos | instancias canónicas |
| `risk` | riesgos | límites y huecos conocidos |
| `rest` | resto | lo no clasificado (**se conserva**) |

Render **texto** para el modelo; render **JSON** para Rust/UI/diff/caché. La
forma es la misma: cambia el envoltorio, no el contenido.

```
nucook/v1 · una relación por línea · fuente <ruta> (sha <8>)
skill rust-optimizer
want  improve performance
when  optimize a rust project
not   the project is not rust
keep  preserve the public api
do    reduce allocations
prove tests pass
```

---

## 3. Los dos guardianes (tests)

1. **No se pierde** — cada unidad con carga del origen queda en la forma
   enseñable **o consta** como ruido/duplicado. Nada desaparece en silencio:
   `unidades == conservadas + ruido + duplicados`.
2. **No hay ruido** — la salida no trae reglas `---`, encabezados de navegación,
   boilerplate (licencia/versión/autor) ni líneas repetidas.

El guardián 1 es la definición honesta de "sin pérdida": no exige igualdad de
bytes, exige que **no haya pérdida invisible**.

---

## 4. Modos de cocción

- **cruda / estructural (implementado)** — determinista, sin modelo: saca el
  ruido de formato, clasifica por sección y por señal (`->`, `must`, `use …`,
  `when …`) y conserva lo dudoso en `rest`.
- **gourmet / semántica (implementado)** — un pase con el modelo (el router de
  NuCoder ya existe) que ataca el ruido **de menos**: cortesía, narración y
  **supuestos implícitos**; además parte los párrafos en unidades. Se cachea por
  `hash` del origen **+ modelo + versión del prompt** (si cambia el prompt, la
  caché se invalida). El prompt define cada etiqueta y trae un *few-shot*.

> **Límite honesto:** el pase determinista fragmenta por línea y clasifica
> grueso en skills con mucha **prosa**; ahí el **gourmet** es el que rinde (en
> REA pasó de fragmentos a `when/not/want/have/do/keep/map` correctos). El
> determinista ya rinde en skills estructurados y sirve de respaldo sin modelo.

---

## 5. CLI

Hay dos entradas: el subcomando `nucoder cook` y el binario propio **`nucook`**
(la CLI de NuCook, literal).

```powershell
# binario propio (recomendado)
nucook list                       # recetas (skills) disponibles
nucook <id>                       # cocina una y la muestra
nucook <id> --gourmet             # pase semántico (modelo, cacheado)
nucook bake [--gourmet]           # cocina TODAS y guarda el **recetario**
nucook cook-dir <dir>             # cocina todos los .md/.txt de un directorio (KB/docs/datasets)
nucook show <id>                  # muestra una receta guardada
nucook ./SKILL.md --save          # un archivo suelto

# o por el CLI de NuCoder (equivalente)
nucoder cook <id|ruta> [--gourmet] [--model <id>] [--refresh] [--json] [--stats]
```

- **Recetario**: `~/.config/nucoder/cookbook/<id>.cookie` (JSON). `nucook bake`
  lo llena; `nucook show <id>` lo lee.
- **Caché del gourmet**: `~/.config/nucoder/cookies/` (clave = `hash` del origen
  + modelo + **versión del prompt**). `--refresh` fuerza a recocinar.
- Compilar el binario: `cargo build --release --bin nucook`
  (`nucook/target/release/nucook.exe`).

---

## 6. Fases

| Fase | Qué | Estado |
|---|---|---|
| **C0** | `nucoder/src/cook.rs`: destilador determinista + render texto/JSON + dos guardianes + CLI `cook` | ✅ |
| **C1** | Caché por `hash` del origen (`~/.config/nucoder/cookies/`) | ✅ |
| **C1b** | Binario propio **`nucook`** (`cook`/`list`/`bake`/`show`) + **recetario** (`~/.config/nucoder/cookbook/*.cookie`) | ✅ |
| **C2** | Agente: anunciar el bloque `nucook` en vez de la descripción; cargar la receta con `skill` | ✅ |
| **C3** | **gourmet**: cocción semántica con el modelo (tarea del router) + caché por prompt | ✅ |
| **C4** | Cocinar **KB/docs/datasets** ✅ (`cook-dir` + comando de paleta *NuCook: cocinar carpeta de docs…*); normalizador **estructural en SpASM** ✅ (`spasm/cook.spasm`) | ✅ |

> La **interfaz** para buscar / descargar / administrar skills está diseñada en
> [`SKILLS_UI.md`](SKILLS_UI.md). El núcleo (`nucoder/src/hub.rs` + `nucook
> search|install|uninstall`) ya está; falta cablear los botones del modal.

---

## 7. Lo que no hay que hacer

- **No inventar vocabulario.** Cada palabra nueva es una capa más.
- **No resumir para achicar.** Achicar no es el objetivo; enseñar es el objetivo.
- **No perder en silencio.** Lo dudoso va a `rest`, no a la basura.
- **No cambiar el idioma.** `v1` y estable: un idioma que cambia no es un puente.
