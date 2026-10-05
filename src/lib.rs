// NuIde — IDE para SpASM con terminales integrados y OpenCode CLI.
//
// Proyecto independiente: no es un plugin de audio ni depende de ningún crate
// compartido del workspace NuToys. Solo usa `slint` para la UI y `arboard` para
// el portapapeles.
//
// Ver `README.md` y `docs/NUIDE_IDEA.md` en esta carpeta.

pub mod build;
pub mod config;
pub mod editor;
pub mod explorer;
pub mod git;
pub mod highlight;
pub mod lint;
pub mod pty;
pub mod search;
pub mod spasm;
pub mod templates;
pub mod terminal;
pub mod ui;
pub mod vt;
