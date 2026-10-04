fn main() {
    slint_build::compile("ui/nuide_ui.slint").expect("failed to compile the NuIde Slint UI");

    println!("cargo:rerun-if-changed=build.rs");
    // Todo el directorio `ui/`: la ventana importa los demás `.slint`.
    println!("cargo:rerun-if-changed=ui");
}
