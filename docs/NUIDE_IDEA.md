# NuIde — idea y especificación

Referencia visual: [`../SpASM Studio con terminales integrados.png`](../SpASM%20Studio%20con%20terminales%20integrados.png)
(boceto original; el nombre del archivo es histórico).

NuIde es un editor de código con **soporte de SpASM** (transpiler + compilador
nativo + lint) **y de otros lenguajes**, terminales integrados y un
agente (OpenCode CLI). La imagen define el diseño visual; los ejemplos de
código, versiones y respuestas de OpenCode son **contenido ilustrativo**, no
especificaciones funcionales.

---

## Ventana

La ventana se divide en tres columnas y dos barras generales:

| Área | Tamaño inicial | Contenido |
|---|---:|---|
| Barra superior | 42 px de alto | Icono y nombre, menú Archivo / Editar / Ver / Ejecutar, nombre del proyecto y controles de ventana |
| Columna izquierda | 18 % del ancho | Explorador de archivos |
| Columna central | 54 % del ancho | Editor de código y terminal inferior |
| Columna derecha | 28 % del ancho | Terminal dedicada a OpenCode CLI |
| Barra inferior | 28 px de alto | Rama Git, lenguaje, codificación y posición del cursor |

Los divisores verticales y el divisor horizontal central **se pueden arrastrar**.
Cada área mantiene su propio scroll; cambiar el tamaño de la terminal inferior
solo afecta al editor central.

### Explorador izquierdo

- Encabezado «EXPLORADOR».
- Árbol de carpetas desplegables, iconos pequeños y nombres de archivos.
- La selección aparece sobre una franja verde petróleo tenue, con esquinas
  redondeadas.
- Ocupa toda la altura disponible entre las barras generales.

### Columna central

- El editor ocupa inicialmente el **75 %** de la altura de la columna.
- Arriba: pestañas con botón de cierre y una **línea menta** sobre la pestaña
  activa; debajo, una ruta de navegación.
- El área de código incluye números de línea, tipografía monoespaciada,
  resaltado de sintaxis y una franja discreta para la línea activa.
- **Zócalo inferior (25 %):** terminal local. Encabezado «TERMINAL», pestañas de
  sesiones (`bash`, `build`), botón para crear otra sesión, cerrar y dividir.
  Esta terminal queda **exclusivamente debajo del editor central**.

### Panel derecho

- Encabezado «OPENCODE CLI» y estado de conexión con un punto verde.
- Salida de terminal: comandos, progreso, propuestas y **diffs** (líneas
  agregadas en verde y eliminadas en rojo suave).
- El prompt está dentro de la terminal. Ocupa toda la altura disponible y
  funciona como terminal interactiva, **sin burbujas de chat**.

---

## Identidad visual

Apariencia de **instrumento electrónico retrofuturista sobrio**: superficies
oscuras, bordes finos con un relieve muy leve, esquinas de 6–8 px, botones
compactos y poco brillo. El código debe seguir siendo el protagonista.

| Uso | Color |
|---|---|
| Fondo general | `#101718` |
| Paneles | `#151D1F` |
| Controles y pestañas | `#202A2C` |
| Bordes | `#69716C` |
| Texto principal marfil | `#E8E2D0` |
| Texto secundario | `#94A09E` |
| Acento menta | `#83D9CB` |
| Acento ámbar | `#E7BA70` |
| Selección | `#294443` |

La tipografía sans es local (`IdeTheme.sans`) y el código usa una pila
monoespaciada del sistema. NuIde no depende de ningún kit compartido.

---

## Mapa a la implementación

| Especificación | Dónde vive |
|---|---|
| Paleta y métricas | `ui/ide_theme.slint` (`IdeTheme`) |
| Modelos (nodos, tokens, líneas) | `ui/ide_types.slint` |
| Barra superior | `ui/ide_topbar.slint` |
| Barra inferior | `ui/ide_statusbar.slint` |
| Explorador | `ui/ide_explorer.slint` |
| Editor (pestañas, ruta, código) | `ui/ide_editor.slint` |
| Terminal local | `ui/ide_terminal.slint` (`IdeTerminal` + `TerminalBody`) |
| OpenCode CLI | `ui/ide_opencode.slint` |
| Layout, divisores y datos demo | `ui/nuide_ui.slint` |
| Puente Rust | `src/editor.rs` |
