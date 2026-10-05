//! NuIde.
//!
//! Arranque en tres pasos: se crea la ventana, se construye el estado de la IDE
//! y se enganchan los callbacks, y se corre. El estado vive en `editor::Ide`;
//! `main` no guarda nada.

use nuide::editor::{self, Ide};
use nuide::ui::NuIdeWindow;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    // `--version` / `--help` no abren ventana: sirven de *smoke test* sin display.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("NuIde {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "NuIde {} — IDE para SpASM con terminales y OpenCode CLI\n\nUso: nuide [--version] [--help]",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    let window = NuIdeWindow::new()?;
    let ide = Ide::shared();
    editor::attach(&window, ide);
    window.run()
}
