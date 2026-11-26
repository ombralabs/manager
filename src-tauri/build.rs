fn main() {
    // generate_context! caches the macOS dev icon; rebuild it when its source changes.
    println!("cargo:rerun-if-changed=icons");
  tauri_build::build()
}
