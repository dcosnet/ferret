// build.rs — generate raw FFI bindings to libmpv via bindgen.
//
// We resolve libmpv through pkg-config. The setup script generates
// mpv-prefix/usr/lib/x86_64-linux-gnu/pkgconfig/mpv.pc and exports
// PKG_CONFIG_PATH via mpv-prefix/env.sh — `cargo build` picks it up
// automatically once env.sh is sourced.
//
// The generated bindings end up in OUT_DIR/bindings.rs and are re-exported
// from `crate::sys`.

use std::env;
use std::path::PathBuf;

fn main() {
    // Re-run if the env or any pkg-config .pc file under the prefix changes.
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    if let Ok(prefix) = env::var("MPV_PREFIX") {
        let pc = PathBuf::from(&prefix)
            .join("usr/lib/x86_64-linux-gnu/pkgconfig/mpv.pc");
        println!("cargo:rerun-if-changed={}", pc.display());
    }

    let mpv = pkg_config::Config::new()
        .atleast_version("2.0")
        .probe("mpv")
        .expect("failed to locate libmpv via pkg-config (did you `source ./mpv-prefix/env.sh`?)");

    // Tell cargo to link libmpv.
    for path in &mpv.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    println!("cargo:rustc-link-lib=dylib=mpv");

    // Embed rpath so the binary finds libmpv.so.2 at runtime without
    // requiring LD_LIBRARY_PATH. We use the first link path from pkg-config.
    if let Some(first) = mpv.link_paths.first() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", first.display());
    }

    // Find the canonical client.h — it #includes the other public headers.
    let client_h = mpv
        .include_paths
        .iter()
        .map(|p| p.join("mpv/client.h"))
        .find(|p| p.exists())
        .expect("could not find mpv/client.h in include paths");

    // Clang's builtin headers (stddef.h etc.) — needed because we ship our
    // own libclang without a system sysroot.
    let clang_resource_dir = env::var("CLANG_RESOURCE_DIR")
        .unwrap_or_else(|_| "/usr/lib/llvm-19/lib/clang/19".to_string());
    let clang_include = format!("{clang_resource_dir}/include");

    let bindings = bindgen::Builder::default()
        .header(client_h.to_string_lossy().into_owned())
        // Pull in the other public headers transitively.
        .clang_arg("-include")
        .clang_arg("mpv/render.h")
        .clang_arg("-include")
        .clang_arg("mpv/render_gl.h")
        .clang_arg("-include")
        .clang_arg("mpv/stream_cb.h")
        // Clang builtin headers (stddef.h, stdarg.h, etc.)
        .clang_arg("-isystem")
        .clang_arg(&clang_include)
        // Allowlist: only the public mpv API surface.
        .allowlist_function("mpv_.*")
        .allowlist_type("mpv_.*")
        .allowlist_var("MPV_.*")
        .allowlist_var("mpv_.*")
        // Layout tests break across glibc versions for some reason; we don't need them.
        .layout_tests(false)
        .derive_default(true)
        .derive_debug(true)
        .fit_macro_constants(false)
        .generate()
        .expect("bindgen failed to generate mpv bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("failed to write bindings.rs");
}
