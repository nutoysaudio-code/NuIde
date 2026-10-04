// NuIde — IDE para SpASM con terminales integrados y OpenCode CLI.
//
// Este crate es deliberadamente delgado: todo lo genérico (motor DSP SpASM,
// `editor_kit`, análisis y el kit de UI Slint "chalk") vive en el crate
// compartido `nutoys-kit` (`../../shared`) y se reutiliza *in place*.
//
// A diferencia de los crates de `plugins/`, este NO es un plugin nice-plug: es
// una aplicación de escritorio (editor de código + terminales + agente). Aun
// así comparte la tipografía y los ingredientes del kit.
//
// Ver `README.md` y `docs/NUIDE_IDEA.md` en esta carpeta.

pub mod editor;
pub mod explorer;
pub mod highlight;
pub mod spasm;
pub mod terminal;
pub mod ui;
