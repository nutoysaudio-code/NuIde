fn main() {
    slint_build::compile("ui/nuide_ui.slint").expect("failed to compile the NuIde Slint UI");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ui/nuide_ui.slint");
    // El kit de UI es compartido con plugins y la otra app: un cambio ahí debe
    // recompilar esta UI.
    println!("cargo:rerun-if-changed=../../shared/ui/kit");
}
