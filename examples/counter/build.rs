//! Builds the helper from the same attributed extern block used by Rust.

fn main() {
    filc_build::build("src/bridge.rs").expect("compile the counter helper with Fil-C");
}
