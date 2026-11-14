use std::{env, fs, path::PathBuf};

fn main() {
    const KEY: &str = "FASTGPT_RUNTIME_MANIFEST";
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/flashgpt.rc");
        println!("cargo:rerun-if-changed=assets/flashgpt.ico");
        embed_resource::compile("assets/flashgpt.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding the flashGPT application icon");
    }
    println!("cargo:rerun-if-env-changed={KEY}");
    let destination = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("runtime.json");
    if let Some(path) = env::var_os(KEY) {
        println!("cargo:rerun-if-changed={}", PathBuf::from(&path).display());
        fs::copy(path, destination).expect("copying the verified runtime manifest");
    } else {
        fs::write(destination, r#"{"runtimes":[]}"#).unwrap();
    }
}
