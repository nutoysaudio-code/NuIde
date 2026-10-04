//! NuIde.
//!
//! Arranque en tres pasos: se crea la ventana, se construye el estado de la IDE
//! y se enganchan los callbacks, y se corre. El estado vive en `editor::Ide`;
//! `main` no guarda nada.

use nuide::editor::{self, Ide};
use nuide::ui::NuIdeWindow;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    let window = NuIdeWindow::new()?;
    let ide = Ide::shared();
    editor::attach(&window, ide);
    window.run()
}
