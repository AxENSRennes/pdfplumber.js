//! `pdftrace`: writes the canonical trace of a pipeline stage, like
//! `oracle/trace.py` does in Python.
//!
//! ```text
//! pdftrace --stage S [--password P] [--max-pages N] FILE.pdf
//!     the trace of FILE.pdf on stdout
//! pdftrace --stage S [--password P] [--max-pages N] --out DIR F1.pdf F2.pdf ...
//!     the trace of each file in DIR/<sha256 of the PDF bytes>.txt
//! pdftrace --list-stages
//! ```
//!
//! `--max-pages` defaults to 20 for page-by-page stages; `0` means every
//! page. Exit codes: 0 on success, 1 when a file cannot be read or written,
//! 2 for a usage error or a stage that is unknown or not implemented yet.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pdftrace::canon::sha256_hex;
use pdftrace::stages::{self, Stage, StageInput};

const DEFAULT_MAX_PAGES: usize = 20;

const USAGE: &str = "usage: pdftrace --stage S [--password P] [--max-pages N] FILE.pdf\n       \
pdftrace --stage S [--password P] [--max-pages N] --out DIR FILE.pdf...\n       \
pdftrace --list-stages";

struct Args {
    stage: String,
    password: String,
    max_pages: Option<usize>,
    out: Option<PathBuf>,
    files: Vec<PathBuf>,
}

fn parse_args(mut argv: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut stage = None;
    let mut password = String::new();
    let mut max_pages = Some(DEFAULT_MAX_PAGES);
    let mut out = None;
    let mut files = Vec::new();
    while let Some(arg) = argv.next() {
        let mut value = |name: &str| argv.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--stage" => stage = Some(value("--stage")?),
            "--password" => password = value("--password")?,
            "--max-pages" => {
                let n: usize = value("--max-pages")?.parse().map_err(|_| "--max-pages needs a number".to_owned())?;
                max_pages = (n > 0).then_some(n);
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--list-stages" => return Ok(None),
            "-h" | "--help" => return Err(String::new()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            file => files.push(PathBuf::from(file)),
        }
    }
    let stage = stage.ok_or("--stage is required")?;
    match (&out, files.len()) {
        (None, 1) | (Some(_), 1..) => {}
        (None, _) => return Err("give one FILE.pdf, or --out DIR with several".into()),
        (Some(_), 0) => return Err("no FILE.pdf given".into()),
    }
    Ok(Some(Args { stage, password, max_pages, out, files }))
}

fn trace(stage: &Stage, args: &Args, pdf: &[u8], out: &mut dyn Write) -> io::Result<()> {
    let run = stage.run.expect("checked by main");
    run(&StageInput { pdf, password: &args.password, max_pages: args.max_pages }, out)
}

/// Writes the trace of `file` to `dir/<sha256>.txt` through a temporary file.
fn trace_to_dir(stage: &Stage, args: &Args, file: &Path, dir: &Path) -> io::Result<()> {
    let pdf = fs::read(file)?;
    let target = dir.join(format!("{}.txt", sha256_hex(&pdf)));
    let tmp = dir.join(format!(".{}.{}.tmp", sha256_hex(&pdf), std::process::id()));
    let mut buf = Vec::new();
    trace(stage, args, &pdf, &mut buf)?;
    fs::write(&tmp, &buf)?;
    fs::rename(&tmp, &target)
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            for stage in stages::STAGES {
                println!("{}{}", stage.name, if stage.run.is_some() { "" } else { " (not implemented)" });
            }
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("pdftrace: {msg}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let Some(stage) = stages::lookup(&args.stage) else {
        eprintln!("pdftrace: unknown stage {:?} (see --list-stages)", args.stage);
        return ExitCode::from(2);
    };
    if stage.run.is_none() {
        eprintln!("pdftrace: stage {:?} is not implemented yet", stage.name);
        return ExitCode::from(2);
    }
    let mut status = ExitCode::SUCCESS;
    match &args.out {
        None => {
            let file = &args.files[0];
            let pdf = match fs::read(file) {
                Ok(pdf) => pdf,
                Err(e) => {
                    eprintln!("pdftrace: {}: {e}", file.display());
                    return ExitCode::from(1);
                }
            };
            let stdout = io::stdout();
            let mut out = io::BufWriter::new(stdout.lock());
            if let Err(e) = trace(stage, &args, &pdf, &mut out).and_then(|()| out.flush()) {
                eprintln!("pdftrace: {e}");
                status = ExitCode::from(1);
            }
        }
        Some(dir) => {
            if let Err(e) = fs::create_dir_all(dir) {
                eprintln!("pdftrace: {}: {e}", dir.display());
                return ExitCode::from(1);
            }
            for file in &args.files {
                if let Err(e) = trace_to_dir(stage, &args, file, dir) {
                    eprintln!("pdftrace: {}: {e}", file.display());
                    status = ExitCode::from(1);
                }
            }
        }
    }
    status
}
