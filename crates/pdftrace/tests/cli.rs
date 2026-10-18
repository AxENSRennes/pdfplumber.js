//! The `pdftrace` command line: `--cmap-dir` (the predefined CMap provider
//! of the process).
//!
//! The stage run here loads no predefined CMap (the `fonts` and `interp`
//! stages do, checked by `oracle.verify` with `PDFPLUMBER_CMAP_DIR`): what
//! is checked here is that a valid
//! directory is accepted without changing a trace, and that an invalid one
//! is a usage error. Runs natively only (WASI has no processes).

#![cfg(not(target_os = "wasi"))]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use pdftrace::typed_json::{self, Json};
use pdftrace::unit::{cache_dir, UnitData};

const PDFTRACE: &str = env!("CARGO_BIN_EXE_pdftrace");

/// A PDF of the repository's micro corpus.
fn micro_pdf() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/micro/encodings-cmap.pdf")
}

/// The compact CMaps the `cmaps` generator wrote (its `cmap_dir`).
fn cmap_dir() -> PathBuf {
    let data = UnitData::open("cmaps");
    let meta = fs::read_to_string(data.path("meta.json")).expect("meta.json");
    let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
    let Some(Json::String(rel)) = meta.get("cmap_dir") else { panic!("meta.json has no cmap_dir") };
    cache_dir().join(rel)
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(PDFTRACE).args(args).env_remove("PDFPLUMBER_CMAP_DIR").output().expect("pdftrace runs")
}

#[test]
fn cmap_dir_is_installed_or_refused() {
    let pdf = micro_pdf();
    let pdf = pdf.to_str().expect("a UTF-8 path");
    let plain = run(&["--stage", "objects", pdf]);
    assert!(plain.status.success(), "{}", String::from_utf8_lossy(&plain.stderr));
    let dir = cmap_dir();
    let dir = dir.to_str().expect("a UTF-8 path");
    for args in [
        vec!["--stage", "objects", "--cmap-dir", dir, pdf],
        vec!["--stage", "objects", &format!("--cmap-dir={dir}"), pdf],
    ] {
        let with = run(&args);
        assert!(with.status.success(), "{}", String::from_utf8_lossy(&with.stderr));
        assert_eq!(with.stdout, plain.stdout, "the provider changes no trace of the stage");
    }
    let env =
        Command::new(PDFTRACE).args(["--stage", "objects", pdf]).env("PDFPLUMBER_CMAP_DIR", dir).output().unwrap();
    assert!(env.status.success());
    assert_eq!(env.stdout, plain.stdout);

    let missing = "/nonexistent/pdfplumber-cmaps";
    for out in [
        run(&["--stage", "objects", "--cmap-dir", missing, pdf]),
        Command::new(PDFTRACE).args(["--stage", "objects", pdf]).env("PDFPLUMBER_CMAP_DIR", missing).output().unwrap(),
    ] {
        assert_eq!(out.status.code(), Some(2));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.starts_with(&format!("pdftrace: --cmap-dir {missing}: ")), "{err}");
        assert!(out.stdout.is_empty());
    }
    let no_value = run(&["--stage", "objects", pdf, "--cmap-dir"]);
    assert_eq!(no_value.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&no_value.stderr).contains("--cmap-dir needs a value"));
}

/// `--oracle-dir` is for the stages that read the oracle's trace
/// (`Stage::oracle_input`): the others refuse it (a usage error).
#[test]
fn oracle_dir_only_for_the_stages_that_read_the_oracle() {
    let pdf = micro_pdf();
    let pdf = pdf.to_str().unwrap();
    let dir = std::env::temp_dir();
    let dir = dir.to_str().unwrap();
    for args in [
        vec!["--stage", "objects", "--oracle-dir", dir, pdf],
        vec!["--stage", "pages", &format!("--oracle-dir={dir}"), pdf],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("does not read the oracle"), "{args:?}");
        assert!(out.stdout.is_empty());
    }
    let listed = run(&["--list-stages"]);
    assert!(String::from_utf8_lossy(&listed.stdout).lines().any(|l| l.starts_with("layout")));
}
