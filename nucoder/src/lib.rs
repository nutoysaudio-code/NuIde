//! Núcleo de **NuCoder**, UI-agnóstico.
//!
//! Aquí vive todo lo que no depende de cómo se dibuja: la config con las API
//! keys (`config`), el router (SpASM AOT vía `ffi`), el cliente HTTP con
//! streaming (`client`), el bucle del agente (`agent`), las herramientas
//! (`tools`), las capacidades de los modelos (`caps`), el filtro de gratis
//! (`freefilter`), la KB (`knowledge`), las imágenes (`image`) y la
//! persistencia de sesiones (`session`).
//!
//! Lo consumen dos frontends:
//!
//! - el CLI `nucoder` (`src/main.rs`): `ask`, `provider`, `models`, `discover`,
//!   `route` y `--version/--help`;
//! - el **panel nativo de NuIde** (Slint), que corre el mismo agente y muestra
//!   la conversación con widgets reales.
//!
//! No tiene dependencias de UI (nada de ratatui/crossterm/Slint).

pub mod agent;
pub mod banner;
pub mod caps;
pub mod client;
pub mod config;
pub mod ffi;
pub mod freefilter;
pub mod image;
pub mod knowledge;
pub mod markdown;
pub mod proc;
pub mod session;
pub mod tools;
