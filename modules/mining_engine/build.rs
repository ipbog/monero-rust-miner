// monero-rust-miner/modules/mining_engine/build.rs
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=randomx_wrapper.h");

    let out_dir = PathBuf::from(env::var("OUT_DIR")
        .map_err(|_| "Переменная окружения OUT_DIR не установлена Cargo.")?);

    let randomx_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?)
        .parent()
        .ok_or("Не удалось получить родительскую директорию для CARGO_MANIFEST_DIR (modules/mining_engine)")?
        .parent()
        .ok_or("Не удалось получить родительскую директорию для CARGO_MANIFEST_DIR (modules)")?
        .join("randomx");

    println!("cargo:rerun-if-changed={}", randomx_root.display());

    let randomx_header_path = randomx_root.join("src").join("randomx.h");
    if !randomx_header_path.exists() {
        let error_message = format!(
            "\nERROR: Исходники RandomX (в частности, randomx.h) не найдены по пути: {}\n\
            Пожалуйста, убедитесь, что Git Submodule RandomX инициализирован и обновлен в корне проекта.\n\
            Выполните команду: 'git submodule update --init --recursive' в директории '{}'.\n\
            Также проверьте права доступа и целостность репозитория RandomX.",
            randomx_header_path.display(),
            randomx_root.parent().map_or_else(|| PathBuf::from("."), |p| p.to_path_buf()).display()
        );
        eprintln!("{}", error_message);
        return Err(Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "RandomX source header not found, see stderr for details.",
        )));
    }

    let src_dir = randomx_root.join("src");

    let c_src_files: Vec<PathBuf> = fs::read_dir(&src_dir)?
        .filter_map(|entry_res| {
            entry_res.ok().and_then(|entry| {
                let path = entry.path();
                if path.is_file() {
                    path.extension().and_then(|ext| {
                        if ext == "c" || ext == "cpp" { Some(path) } else { None }
                    })
                } else { None }
            })
        })
        .collect();

    if c_src_files.is_empty() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No RandomX C/C++ source files found in {}", src_dir.display()),
        )));
    }

    let mut build = cc::Build::new();
    build.cpp(true).flag_if_supported("-std=c++17").flag_if_supported("-pthread");

    if cfg!(target_arch = "x86_64") && !cfg!(feature = "portable_build") {
        build.flag_if_supported("-march=native");
        build.flag_if_supported("-maes");
    } else if cfg!(feature = "portable_build") {
        println!("cargo:warning=Сборка RandomX в режиме portable_build (без специфичных для CPU оптимизаций).");
    }

    build.flag_if_supported("-Wall").warnings(true).include(&src_dir);
    for file in &c_src_files { build.file(file); }
    build.compile("randomx_static_lib");

    println!("cargo:rustc-link-lib=static=randomx_static_lib");
    println!("cargo:rustc-link-search=native={}", out_dir.display());

    let bindings = bindgen::Builder::default()
        .header(PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("randomx_wrapper.h").to_str().ok_or("Invalid path to randomx_wrapper.h")?)
        .clang_arg(format!("-I{}", src_dir.display()))
        .derive_default(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .allowlist_function("randomx_get_flags")
        .allowlist_function("randomx_alloc_cache")
        .allowlist_function("randomx_init_cache")
        .allowlist_function("randomx_destroy_cache")
        .allowlist_function("randomx_alloc_dataset")
        .allowlist_function("randomx_dataset_item_count")
        .allowlist_function("randomx_init_dataset")
        .allowlist_function("randomx_destroy_dataset")
        .allowlist_function("randomx_create_vm")
        .allowlist_function("randomx_destroy_vm")
        .allowlist_function("randomx_vm_set_cache")
        .allowlist_function("randomx_vm_set_dataset")
        .allowlist_function("randomx_calculate_hash")
        .allowlist_type("randomx_flags")
        .allowlist_type("randomx_cache")
        .allowlist_type("randomx_dataset")
        .allowlist_type("randomx_vm")
        .allowlist_var("RANDOMX_FLAG_DEFAULT")
        .allowlist_var("RANDOMX_FLAG_LARGE_PAGES")
        .allowlist_var("RANDOMX_FLAG_HARD_AES")
        .allowlist_var("RANDOMX_FLAG_FULL_MEM")
        .allowlist_var("RANDOMX_FLAG_JIT")
        .allowlist_var("RANDOMX_FLAG_SECURE")
        .allowlist_var("RANDOMX_FLAG_ARGON2_SSSE3")
        .allowlist_var("RANDOMX_FLAG_ARGON2_AVX2")
        .allowlist_var("RANDOMX_FLAG_ARGON2")
        .allowlist_var("RANDOMX_FLAG_SOFT_AES")
        .allowlist_var("RANDOMX_HASH_SIZE")
        .allowlist_var("RANDOMX_DATASET_ITEM_SIZE")
        .blocklist_item("randomx_release_memory")
        .generate()
        .map_err(|e| format!("Bindgen failed: {:?}", e))?;

    bindings.write_to_file(out_dir.join("randomx_ffi.rs"))?;
    Ok(())
}
