//! The shadow stack of the `pdftrace` binary and of the replays compiled to
//! WebAssembly (`wasm32-wasip1`): the same 8 MiB as the JavaScript package's
//! module (`crates/wasm/build.rs`, which says why), so that the traces and
//! replays run on WebAssembly reach Python's recursion limits as the
//! library does.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo::rustc-link-arg=-zstack-size=8388608");
    }
}
