fn main() {
    // Hand the linker our memory layout, and rebuild when it changes — otherwise an edit to
    // the layout silently produces a stale image that still boots, which is a miserable bug.
    println!("cargo:rustc-link-arg=-Tkernel/linker.ld");
    println!("cargo:rerun-if-changed=kernel/linker.ld");
}
