use std::path::PathBuf;
use std::process::Command;

/// Ensures the generated frontend shell (`../src/index.html`) exists before
/// `tauri_build` validates `frontendDist`.
///
/// The desktop shell is assembled from `../src/index.template.html` plus
/// `../src/partials/*.html` by `../build-html.mjs` (see `npm run build:html`).
/// The npm scripts already regenerate it, so this is a safety net for builds
/// that bypass npm (`cargo tauri build`, `cargo build` inside `src-tauri`).
fn ensure_frontend() {
    let frontend = PathBuf::from("../src");
    let template = frontend.join("index.template.html");
    let output = frontend.join("index.html");

    println!("cargo:rerun-if-changed={}", template.display());
    println!(
        "cargo:rerun-if-changed={}",
        frontend.join("partials").display()
    );

    match Command::new("node").arg("build-html.mjs").current_dir("..").status() {
        Ok(status) if status.success() => {}
        Ok(status) => panic!(
            "node build-html.mjs failed with status {status}; \
             run `npm run build:html` inside desktop/ to regenerate the frontend shell"
        ),
        Err(err) => {
            if output.is_file() {
                println!(
                    "cargo:warning=node unavailable ({err}); using the existing \
                     generated frontend shell at {}",
                    output.display()
                );
            } else {
                panic!(
                    "node is required to assemble the Coral Desktop frontend shell ({err}); \
                     install Node.js and run `npm run build:html` inside desktop/"
                );
            }
        }
    }
}

fn main() {
    ensure_frontend();
    tauri_build::build()
}
