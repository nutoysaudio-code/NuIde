fn main() {
    slint_build::compile("ui/nuide_ui.slint").expect("failed to compile the NuIde Slint UI");

    println!("cargo:rerun-if-changed=build.rs");
    // Todo el directorio `ui/`: la ventana importa los demás `.slint`.
    println!("cargo:rerun-if-changed=ui");

    #[cfg(windows)]
    embed_resources();
}

/// Compila `res/nuide.rc` (icono + versión) y lo enlaza, si hay `rc.exe`.
///
/// No agrega dependencias: busca `rc.exe` en la variable `RC` o en los Windows
/// Kits. Si no está, avisa y sigue (el `.exe` se compila sin icono).
#[cfg(windows)]
fn embed_resources() {
    use std::path::PathBuf;

    let rc = PathBuf::from("res/nuide.rc");
    if !rc.is_file() {
        return;
    }
    println!("cargo:rerun-if-changed=res/nuide.rc");
    println!("cargo:rerun-if-changed=res/nuide.ico");

    // El linker MSVC acepta un `.res` directamente como argumento.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let Some(rc_exe) = find_rc() else {
        println!("cargo:warning=rc.exe no encontrado: el .exe no tendrá icono ni versión");
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let res = out.join("nuide.res");
    let status = std::process::Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc)
        .status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg={}", res.display()),
        _ => println!(
            "cargo:warning=no se pudo compilar res/nuide.rc con {}",
            rc_exe.display()
        ),
    }
}

/// Busca `rc.exe` (variable `RC` o el Windows Kit más nuevo).
#[cfg(windows)]
fn find_rc() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(rc) = std::env::var_os("RC") {
        let p = PathBuf::from(rc);
        if p.is_file() {
            return Some(p);
        }
    }
    let base = r"C:\Program Files (x86)\Windows Kits\10\bin";
    let mut best: Option<(String, PathBuf)> = None;
    if let Ok(rd) = std::fs::read_dir(base) {
        for entry in rd.flatten() {
            let version = entry.file_name().to_string_lossy().into_owned();
            let candidate = entry.path().join("x64").join("rc.exe");
            if candidate.is_file() && best.as_ref().is_none_or(|(v, _)| version > *v) {
                best = Some((version, candidate));
            }
        }
    }
    best.map(|(_, path)| path)
}
