//! The shadow stack of the WebAssembly module (`docs/architecture.md`,
//! "Stack Size"): the interpreter recurses in Rust where
//! Python recurses (`execute` -> `do_Do` -> `render_contents` for each
//! nested Form XObject), down to Python's recursion limit. The deepest
//! chain Python accepts takes about 1.2 MiB of the `dist` build's stack
//! (measured: 3.7 KiB per nesting level, about 330 levels), more than the
//! 1 MiB `wasm-ld` reserves by default; 8 MiB leaves a margin of more than
//! six. `scripts/wasm-depth-gate.sh` runs those chains on the `dist` build.
//! A link argument rather than a `.cargo/config.toml` rustflag: `RUSTFLAGS`
//! in the environment would silently replace the latter.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo::rustc-link-arg=-zstack-size=8388608");
    }
}
