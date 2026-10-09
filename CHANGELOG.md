# Changelog

Todos los cambios relevantes de NuIde. El formato sigue
[Keep a Changelog](https://keepachangelog.com/es/1.1.0/) y el proyecto usa
versionado semántico.

## [0.2.2] — 2026-10-08

### Agregado

- **Proveedores de OpenCode (CRUD).** Nueva sección **Proveedores** en la
  configuración: lista los proveedores del CLI y permite **editar, crear y
  eliminar**. Trabaja sobre la config global
  (`~/.config/opencode/opencode.json(c)`) como la **unión** de `opencode.json`
  y `opencode.jsonc`: al guardar escribe todo en `opencode.json` (lo crea si
  falta) y retira `providers` del `.jsonc` para no dejar copias. La escritura es
  **quirúrgica** (sólo se reemplaza el fragmento `providers`), así que se
  conservan `$schema`, `model` y las claves desconocidas de cada proveedor
  (`env`, `modelID`, `limit`, `cost`, …). Los **modelos** se editan como JSON
  crudo y se validan al guardar; eliminar pide confirmación.
- **Minimizar / Maximizar** la ventana desde el menú de la barra superior.
- `src/jsonx.rs`: utilidades JSON con *spans* (tolerantes a comentarios JSONC) y
  validador, base del CRUD.
- **Menú contextual en las terminales** (clic derecho): **Copiar**, **Pegar** y
  **Elevar a administrador**. Vale para la shell y para el panel de OpenCode CLI.
- **Elevación (Windows):** la paleta suma *Terminal: nueva como administrador*
  (shell elevada en ventana aparte, vía UAC) y *NuIde: reiniciar como
  administrador* (relanza toda la IDE elevada). El pie muestra un distintivo
  **ADMIN** cuando la app corre elevada.

### Cambiado

- **Menú superior:** se quita el botón de «Ver» duplicado (hamburguesa); el menú
  queda sólo en la barra de menús y el ítem **Ver** describe el dock de cada
  panel.
- **Ayuda → Ver repositorio** abre el **remoto real** del proyecto
  (`git remote get-url origin`), con el repositorio de NuIde como respaldo. Antes
  apuntaba a GitLab y daba 403.
- **Renderer por defecto:** **FemtoVG** en vez del software, porque el software
  se transparentaba al maximizar. Se puede volver con
  `SLINT_BACKEND=winit-software`.
- **Terminales:** `vt.rs` usa un hash FNV-1a y un flag *dirty*; si no hay salida
  nueva ni cambió la selección, no recalcula filas.
- **Acerca de:** rediseño del diálogo (créditos y versión).
- Título de la ventana `NuIde` y divisores transparentes.

### Corregido

- CI (`.gitlab-ci.yml`) en verde: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings` y `cargo test` (159 pruebas OK).
