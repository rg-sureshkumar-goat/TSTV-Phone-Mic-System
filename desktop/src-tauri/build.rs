use std::path::{Path, PathBuf};

fn main() {
    build_opus();
    tauri_build::build();
}

/// Compiles the vendored libopus (third_party/opus/libopus) into a static library.
fn build_opus() {
    // Not canonicalized: on Windows that gives a \\\\?\\ path, which the MSVC compiler can't use.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/opus/libopus");
    assert!(root.join("include/opus.h").exists(), "third_party/opus/libopus is missing");

    let mut build = cc::Build::new();
    for dir in ["celt", "silk", "silk/float", "src"] {
        for file in c_files(&root.join(dir)) {
            build.file(file);
        }
    }
    build
        .include(root.join("include"))
        .include(&root)
        .include(root.join("celt"))
        .include(root.join("silk"))
        .include(root.join("silk/float"))
        .define("OPUS_BUILD", None)
        .define("HAVE_LRINTF", None)
        .define("HAVE_LRINT", None)
        .opt_level(2)
        .warnings(false);
    // MSVC has no C99 variable-length arrays.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.define("USE_ALLOCA", None);
    } else {
        build.define("VAR_ARRAYS", None);
    }
    build.compile("opus");
    println!("cargo:rerun-if-changed={}", root.display());
}

fn c_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
        .collect();
    files.sort();
    files
}
