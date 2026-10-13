# Guía de NuIde

Guía rápida para usar NuIde —con proyectos **SpASM** y con otros lenguajes—. Para
el detalle interno y las
decisiones de diseño, ver [`ESTADO.md`](ESTADO.md) y [`PLAN.md`](PLAN.md).

## 1. Requisitos

- **SpASM**: el compilador `spasmc.py` + Python y un **compilador de C**
  (`gcc`/`clang`/`tcc` o **MSVC**) — NuIde **compila** el `.spasm` a binario
  nativo (AOT) con Ejecutar (`F5`) y Compilar (`Ctrl+B`); el intérprete
  `spasm.exe` ya no se usa en la IDE. NuIde descubre las herramientas en el
  `PATH`, en `F:\SpASM` / `F:\SpAMS-Oficial`, o por variables de entorno
  (`NUIDE_SPASMC`, `NUIDE_PYTHON`, `NUIDE_CC`). Si falta alguna, lo dice y
  sigue funcionando como editor/terminal.
- **Git** (opcional): el panel de control de código usa el `git` del `PATH`.
- **Node.js** (opcional): lo necesitan las CLI de IA (NuCoder y las demás). El
  instalador puede ponerlo.

## 2. Primer arranque

1. Al abrir sin archivos verás la **bienvenida**: **Abrir carpeta…** (elige el
   proyecto), **Abrir archivo…** o **Nuevo archivo…**.
2. El nombre del proyecto (arriba, al centro) también abre el selector de carpeta.
   Al cambiarlo, el explorador se reconstruye y **el panel derecho se reinicia en
   la carpeta nueva** (como terminal).
3. El editor recuerda la carpeta, el layout, las pestañas abiertas y el cursor de
   cada archivo.

## 3. Editor

- Clic en un archivo del **Explorador** para abrirlo. **Clic derecho** en una fila:
  nuevo archivo/carpeta, renombrar, eliminar, copiar ruta, mostrar en el SO,
  refrescar. En el encabezado, **⟳** refresca y **◍** oculta los generados
  (`target`, `node_modules`, …).
- Edición con resaltado por lenguaje, deshacer/rehacer, autocierre de pares y
  autoindentación. El cursor de cada pestaña se recuerda.
- **Buscar/Reemplazar** (`Ctrl+F` / `Ctrl+H`): mayúsculas (`Aa`), palabra completa
  (`Palabra`), **regex** (`.*`) y **solo en la selección** (`Sel.`). `↑`/`↓`
  recorren el historial.
- **Ir a línea** (`Ctrl+G`) e **Ir a definición** (`F12`) desde el esquema del
  parser.

## 4. Ejecutar y compilar

NuIde **compila** el código SpASM a binario nativo (AOT); el intérprete
`spasm.exe` no participa:

- **`F5` (Ejecutar)** compila el archivo de **entrada** (o el activo) y ejecuta
  el binario con los **argumentos** configurados.
- **`Ctrl+B` (Compilar)** transpila a C (`spasmc.py --target c`) y **compila el
  `.c` a binario** con el compilador de C detectado (`gcc`/`clang`/`cc`/`tcc` o
  **MSVC** vía `vcvars64.bat`); **`Ctrl+Shift+B`** compila **y ejecuta** el
  binario. Si no hay compilador de C, avisa y deja el `.c`.
- Lo mismo vale para el **script de despliegue** (`deploy.spasm`): se compila
  AOT y se corre.
- La entrada/argumentos se cambian desde la paleta: *Ejecutar: elegir archivo de
  entrada…*, *usar el archivo activo*, *argumentos…* y *compilar al guardar*.
- La salida va a la sesión **build** de la terminal; los errores se marcan en el
  editor y **`F8` / `Shift+F8`** saltan entre diagnósticos.
- Las **plantillas** reflejan el flujo: *desarrollo (simple)* para `F5` y
  *release (programa)* para `Ctrl+B`.

## 5. Terminales

- La terminal inferior es una **shell real** (ConPTY). `+` abre sesiones, `×`
  cierra, **⬒** divide en dos vistas (clic en una le da el foco).
- Rueda y `Shift+PageUp/Down` recorren el **scrollback**; arrastrar **selecciona**
  y copia; `Ctrl+Shift+C`/`V` copian/pegan.
- Si un programa pide el mouse (`vim`, la TUI de NuCoder…), los clics y la rueda
  se le **reenvían**. En ese caso **mantené `Shift` mientras arrastrás para
  seleccionar** (y se copia al soltar); el **botón del medio pega** el
  portapapeles. Vale igual para el panel derecho.
- La paleta permite **renombrar** la sesión y **reiniciarla** (arranca en la raíz
  del proyecto).
- El panel derecho es una **terminal igual**: arranca como shell y **`nucoder`
  se lanza bajo demanda** (paleta → *Lanzar nucoder*). Si salís de nucoder,
  la terminal queda en la shell.
- **Clic derecho** en cualquier terminal abre un menú con **Copiar**, **Pegar** y
  **Elevar a administrador** (abre una shell elevada con UAC en la raíz del
  proyecto). En Windows, *NuIde: reiniciar como administrador* (paleta) relanza
  toda la IDE elevada —entonces las terminales del panel también lo son— y el pie
  muestra **ADMIN**.
- En el **panel del agente (coder)** ese menú suma los **atajos de los comandos**
  de NuCoder: las **tareas** (`/complete`, `/refactor`, `/debug`, `/explain`,
  `/test`, `/architect`) se **insertan** en la entrada para que escribas el pedido
  y las **acciones** (`/free`, `/clear`, `/help`, `/quit`) se **ejecutan** al
  toque. La terminal local conserva **Elevar a administrador**.

## 6. Git

- El panel **CÓDIGO** muestra la rama, `↑ahead ↓behind` y los cambios en
  **CAMBIOS** y **PREPARADOS**. **Clic en una fila abre el diff**.
- Botones: **Preparar todo**, **Quitar todo**, **Bajar** (pull), **Ramas**,
  **Push** y **Commit** (con el mensaje del campo). **⟳** relee.
- El nombre de la rama es **clickeable**: cambia de rama o **Nueva rama…**.
- Desde el diff: **Abrir archivo**, **Preparar/Quitar** y **Descartar cambios**
  (con confirmación). Enmendar el último commit, desde la paleta.

## 7. Buscar en el proyecto

- **`Ctrl+Shift+F`** abre el panel: busca en **todos los archivos** (literal o
  regex), muestra `ruta:línea  texto` y **`Enter`/clic abren el archivo**.
- **Reemplazar todo** (con confirmación) aplica el cambio y recarga las pestañas
  abiertas.

## 8. Paleta, preferencias y ayuda

- **`Ctrl+Shift+P`** paleta de comandos; **`Ctrl+P`** buscar archivos (por ruta).
- **`F1`** ayuda de sintaxis SpASM (lista palabras clave/builtins filtradas por la
  palabra bajo el cursor; al elegir, la inserta).
- **`Ctrl+,`** Preferencias: fuentes, tamaño del código, acento, **opacidad de los
  paneles**, tab, compilar al guardar, **Proveedores de NuCoder** y **atajos**
  (clic en uno y presioná la combinación nueva).
- **Reordenar la ventana**: arrastrá el **asa** (las tres rayitas) de la cabecera
  de cualquier panel y soltala sobre la banda **Izquierda / Derecha / Abajo**;
  soltar en el centro lo **oculta**. Se recuerda entre ejecuciones.
- Menú **Ayuda → Acerca de NuIde** (versión y créditos) y **Ver repositorio**
  (abre el remoto del proyecto en el navegador; si no hay, el repo de NuIde).
- El pie muestra, discreto, el uso de **CPU y RAM**.

## 9. Atajos

| Atajo | Acción |
|---|---|
| `F5` / `Ctrl+B` / `Ctrl+Shift+B` | Ejecutar / Compilar / Compilar y ejecutar |
| `F6` / `F8` / `Shift+F8` | Analizar / error siguiente / anterior |
| `F12` | Ir a definición |
| `Ctrl+S` / `Ctrl+Shift+S` / `Ctrl+W` | Guardar / Guardar como / Cerrar pestaña |
| `Ctrl+F` / `Ctrl+H` / `Ctrl+G` | Buscar / Reemplazar / Ir a línea |
| `Ctrl+Shift+F` | Buscar en el proyecto |
| `Ctrl+Shift+P` / `Ctrl+P` | Paleta de comandos / Buscar archivos |
| `Ctrl+Shift+M` | Vista previa de Markdown |
| `Ctrl+,` / `F1` | Preferencias / Ayuda |

Los atajos de la paleta, búsqueda de archivos, buscar en el proyecto y ayuda se
pueden reasignar desde Preferencias.

## 10. Desplegar el sitio

- **`Ctrl+,` → Despliegue**: cargá Host, Usuario, Contraseña, Carpeta remota y, si
  corresponde, Subcarpeta local (p. ej. `build`) y Transporte (FTP / FTPS / SFTP).
  Se guarda **por proyecto** en el config de NuIde, no en el repositorio.
- **Probar conexión** lista el directorio remoto con `curl` sin subir nada.
- **Subir cambios** corre `deploy.spasm` si el proyecto lo tiene (o sube la
  subcarpeta filtrando por extensión) y muestra el progreso en la terminal
  `build`.
- **Terminal SSH** abre una sesión `ssh user@host` en una pestaña de terminal.
- El script decide qué se sube con líneas simples:

  ```text
  UPLOAD css/site.css
  UPLOAD build/app.js public/app.js
  SKIP build/grande.wasm pesado
  LOG 3 archivos pendientes
  ```

## 11. Módulos

Se activan en **Preferencias (`Ctrl+,`) → Módulos**.

- **Servidor web local**: prendelo y abrí `http://localhost:<puerto>/` en el
  navegador. **Puerto**, **Carpeta servida** (por ejemplo `build` o `dist`) y
  **CORS: origen permitido** se editan en esa misma lista. Con `*` cualquier
  origen puede leer el servidor; si la página manda credenciales, poné el dominio
  exacto. Cada pedido aparece en la terminal `build` con su código y tamaño.
- **Visor de SQLite**: al activarlo pide un archivo `.db`/`.sqlite`; el panel
  muestra las tablas a la izquierda y hasta 200 filas a la derecha. **Refrescar**
  vuelve a leer la base. Necesita `sqlite3.exe` o Python en la máquina (se avisa
  si no hay ninguno).

## 12. Proveedores de NuCoder

En **Preferencias (`Ctrl+,`) → Proveedores** están los proveedores del CLI de
NuCoder. La lista muestra cada proveedor y una fila **Agregar proveedor**; al
clic en uno se abre el formulario:

- **ID**: la clave del proveedor en la config; cambiarlo lo **renombra**.
- **Nombre**, **Package**, **baseURL** y **apiKey** (oculta).
- **Modelos**: el objeto JSON tal cual (se valida al guardar; tiene que ser un
  objeto `{ … }`).

**Guardar** escribe la config global de NuCoder
(`~/.config/nucoder/nucoder.json`), uniendo lo que hubiera en `nucoder.jsonc`
(el `.jsonc` queda sin `providers`). La edición es quirúrgica: no toca `$schema`,
`model` ni las claves del proveedor que la IDE no muestra (`env`, `modelID`,
`limit`, `cost`…). Si un proveedor usa variables de entorno (`env`) y dejás la
apiKey vacía, no se escribe una apiKey vacía encima. **Eliminar** pide
confirmación y **Cancelar** / **Esc** cierran sin guardar.

> Conviene tener un respaldo de `~/.config/nucoder` antes de editar: los
> archivos contienen credenciales.

## 13. Agente: skills, NuCook y MCP

El panel derecho corre **NuCoder** (el agente). Tres capacidades nuevas:

- **Skills** (*Skills: abrir catálogo*, paleta o menú **Agente**): lista los
  skills de Claude, OpenCode, `.agents` y los **nativos** de SpASM. Se puede
  **buscar** (local y, con `NUCOOK_CATALOG`, en la web), **instalar/actualizar**
  (`owner/repo` o una ruta), **cocinar**, **eliminar** y **anunciar/ocultar**
  (permisos `allow/ask/deny`). En el chat, `@id` precarga un skill.
- **NuCook**: destila la **habilidad** de un skill (sin ruido humano) en su forma
  enseñable; el agente la usa si está en el **recetario**. La paleta tiene
  *NuCook: cocinar carpeta de docs…* para KB/docs/datasets. La CLI es `nucook`
  (`nucook list|search|install|cook-dir|bake`), instalada con NuIde.
- **MCP**: configurá `mcp.servers` en `~/.config/nucoder/config.json` (p. ej.
  REA) y sus herramientas aparecen como `<servidor>__<tool>`. Guías:
  [`SKILLS_UI.md`](SKILLS_UI.md), [`NUCOOK.md`](NUCOOK.md) y
  [`MCP.md`](MCP.md).

## 14. Problemas frecuentes

- **«no se encontró spasmc.py»**: definí `NUIDE_SPASMC`/`NUIDE_PYTHON` o instalá
  el toolchain de SpASM.
- **Compilar no llega al binario**: falta un compilador de C (`gcc`/`clang`/`cc`
  o MSVC); sin él, `spasmc` solo transpila a `.c` y NuIde lo avisa.
- **El instalador de SpASM no trajo `spasmc.py` completo**: NuIde usa el que
  descubre (`NUIDE_SPASMC`/`SPASMC`).
