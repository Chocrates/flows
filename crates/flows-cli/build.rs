//! Force a rebuild when any embedded asset changes. `include_dir!` (used in
//! `src/`) inlines files at compile time, but Cargo doesn't track the macro's
//! file dependencies — so without this, adding or editing a template/prompt/
//! fixture would silently keep the stale embedded copy. Emitting
//! `rerun-if-changed` for the whole `assets/` tree fixes that.
use std::path::Path;

fn main() {
    watch(Path::new("assets"));
}

fn watch(dir: &Path) {
    if !dir.exists() {
        return;
    }
    // On a directory this catches file adds/removes; on each file, edits.
    println!("cargo:rerun-if-changed={}", dir.display());
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            println!("cargo:rerun-if-changed={}", path.display());
            if path.is_dir() {
                watch(&path);
            }
        }
    }
}
