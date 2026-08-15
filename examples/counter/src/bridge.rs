//! Safe functions generated from a Rust-shaped Fil-C foreign interface.

#[filc::bridge(
    name = "counter_demo",
    header = "legacy.h",
    sources = ["legacy.c"],
    includes = ["."],
)]
unsafe extern "Fil-C" {
    #[link_name = "legacy_add"]
    pub fn add(left: i32, right: i32) -> i32;

    #[link_name = "legacy_reverse"]
    #[filc::free("legacy_release_bytes")]
    pub fn reverse(input: &[u8]) -> Vec<u8>;

    #[link_name = "legacy_greet"]
    #[filc::free("legacy_release_string")]
    pub fn greet(name: &str) -> String;

    pub fn counter_new(initial: i64) -> *mut counter_t;

    pub fn counter_add(counter: *mut counter_t, delta: i64) -> i64;

    #[filc::drop]
    pub fn counter_drop(counter: *mut counter_t);

    #[link_name = "legacy_trigger_oob"]
    pub fn trigger_oob(input: &[u8]) -> u32;
}
