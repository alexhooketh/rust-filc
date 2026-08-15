//! Build-script support for generating and compiling a whole-program Fil-C
//! helper plus its safe Rust client bindings.

#![forbid(unsafe_code)]

mod c;
mod rust;
mod schema;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::schema::{Schema, shouty};

/// A code-generation or Fil-C compiler failure.
#[derive(Debug)]
pub enum Error {
    /// A required Cargo build-script environment variable is absent.
    MissingEnvironment(&'static str),
    /// No Fil-C compiler was explicitly configured.
    MissingCompiler,
    /// A schema is malformed or unsupported.
    Schema(String),
    /// A source or generated file could not be read or written.
    Io(io::Error),
    /// The configured Fil-C compiler returned a failure status.
    CompilerFailed(Option<i32>),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnvironment(name) => {
                write!(formatter, "missing build environment variable {name}")
            }
            Self::MissingCompiler => write!(
                formatter,
                "FILC_CC is unset; point it at a Fil-C clang executable (ordinary C compilers are never used as a fallback)"
            ),
            Self::Schema(message) => write!(formatter, "invalid Fil-C bridge schema: {message}"),
            Self::Io(error) => write!(formatter, "Fil-C bridge build I/O failed: {error}"),
            Self::CompilerFailed(Some(code)) => {
                write!(formatter, "Fil-C compiler failed with exit code {code}")
            }
            Self::CompilerFailed(None) => {
                write!(formatter, "Fil-C compiler terminated without an exit code")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::MissingEnvironment(_)
            | Self::MissingCompiler
            | Self::Schema(_)
            | Self::CompilerFailed(_) => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Configuration used from a dependent crate's `build.rs`.
#[derive(Debug)]
pub struct Config {
    declaration: PathBuf,
    compiler: Option<PathBuf>,
    compiler_args: Vec<OsString>,
}

impl Config {
    /// Starts a bridge build from one Rust file containing an attributed
    /// `unsafe extern "Fil-C"` block.
    #[must_use]
    pub fn new(declaration: impl Into<PathBuf>) -> Self {
        Self {
            declaration: declaration.into(),
            compiler: None,
            compiler_args: Vec::new(),
        }
    }

    /// Uses an explicit Fil-C compiler instead of `FILC_CC`.
    #[must_use]
    pub fn compiler(mut self, compiler: impl Into<PathBuf>) -> Self {
        self.compiler = Some(compiler.into());
        self
    }

    /// Appends one compiler or linker argument.
    #[must_use]
    pub fn compiler_arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.compiler_args.push(argument.as_ref().to_owned());
        self
    }

    /// Generates both peers and compiles the complete helper with Fil-C.
    ///
    /// When no compiler is configured, rust-analyzer invocations generate the
    /// bindings without a helper so language-server analysis can proceed. An
    /// ordinary Cargo build still returns [`Error::MissingCompiler`].
    pub fn build(self) -> Result<Artifacts, Error> {
        let manifest = env::var_os("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .ok_or(Error::MissingEnvironment("CARGO_MANIFEST_DIR"))?;
        let output = env::var_os("OUT_DIR")
            .map(PathBuf::from)
            .ok_or(Error::MissingEnvironment("OUT_DIR"))?;
        let declaration_path = resolve(&manifest, &self.declaration);
        let declaration_source = fs::read_to_string(&declaration_path)?;
        let schema = Schema::parse(&declaration_source).map_err(Error::Schema)?;
        let hash: [u8; 32] = Sha256::digest(schema.canonical().as_bytes()).into();
        let stem = &schema.bridge.name;
        let rust_path = output.join(format!("{stem}.rs"));
        let c_path = output.join(format!("{stem}_server.c"));
        let header_path = output.join("extern_filc.h");
        let program_path = output.join(format!("{stem}_helper"));

        fs::write(&rust_path, rust::generate(&schema, &hash))?;
        fs::write(&c_path, c::generate(&schema, &hash))?;
        fs::write(&header_path, c::header())?;

        println!("cargo:rerun-if-env-changed=FILC_CC");
        println!("cargo:rerun-if-env-changed=EXTERN_FILC_ANALYSIS");
        println!("cargo:rerun-if-env-changed=RUSTC_WRAPPER");
        println!("cargo:rerun-if-env-changed=RUSTC_WORKSPACE_WRAPPER");
        println!("cargo:rerun-if-changed={}", declaration_path.display());
        println!(
            "cargo:rerun-if-changed={}",
            resolve(&manifest, Path::new(&schema.bridge.header)).display()
        );
        for source in &schema.bridge.sources {
            println!(
                "cargo:rerun-if-changed={}",
                resolve(&manifest, Path::new(source)).display()
            );
        }
        let env_name = format!("EXTERN_FILC_{}_PROGRAM", shouty(&schema.bridge.name));
        println!("cargo:rustc-env={env_name}={}", program_path.display());

        let compiler = self.compiler.or_else(|| {
            env::var_os("FILC_CC")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        });
        if compiler.is_none() && analysis_requested() {
            return Ok(Artifacts {
                bindings: rust_path,
                helper: program_path,
                c_dispatcher: c_path,
                c_header: header_path,
            });
        }
        let compiler = compiler.ok_or(Error::MissingCompiler)?;
        let mut command = Command::new(compiler);
        command
            .arg("-std=c17")
            .arg("-O2")
            .arg("-g")
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-Wno-unused-function")
            .arg("-I")
            .arg(&output);
        for include in &schema.bridge.includes {
            command
                .arg("-I")
                .arg(resolve(&manifest, Path::new(include)));
        }
        command.arg(&c_path);
        for source in &schema.bridge.sources {
            command.arg(resolve(&manifest, Path::new(source)));
        }
        command
            .args(&self.compiler_args)
            .arg("-o")
            .arg(&program_path);
        let status = command.status()?;
        if !status.success() {
            return Err(Error::CompilerFailed(status.code()));
        }

        Ok(Artifacts {
            bindings: rust_path,
            helper: program_path,
            c_dispatcher: c_path,
            c_header: header_path,
        })
    }
}

/// Builds the bridge declared in `declaration` using default configuration.
///
/// This is the intended one-line build-script entrypoint. Use [`Config`] when
/// an explicit compiler or additional compiler arguments are required.
pub fn build(declaration: impl Into<PathBuf>) -> Result<Artifacts, Error> {
    Config::new(declaration).build()
}

/// Paths generated by a successful bridge build.
#[derive(Debug)]
pub struct Artifacts {
    /// Rust source to include from the dependent crate.
    pub bindings: PathBuf,
    /// Complete Fil-C helper executable.
    pub helper: PathBuf,
    /// Generated C dispatcher source.
    pub c_dispatcher: PathBuf,
    /// Boundary types included by the legacy adapter header.
    pub c_header: PathBuf,
}

fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

fn analysis_requested() -> bool {
    env::var_os("EXTERN_FILC_ANALYSIS").is_some_and(|value| value == "1")
        || env::var_os("RUSTC_WRAPPER").is_some_and(|value| is_rust_analyzer_wrapper(&value))
        || env::var_os("RUSTC_WORKSPACE_WRAPPER")
            .is_some_and(|value| is_rust_analyzer_wrapper(&value))
}

fn is_rust_analyzer_wrapper(value: &OsStr) -> bool {
    Path::new(value)
        .file_stem()
        .is_some_and(|name| name == "rust-analyzer")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::is_rust_analyzer_wrapper;

    #[test]
    fn recognizes_only_the_rust_analyzer_wrapper() {
        assert!(is_rust_analyzer_wrapper(OsStr::new("rust-analyzer")));
        assert!(is_rust_analyzer_wrapper(OsStr::new(
            "/opt/editor/bin/rust-analyzer"
        )));
        assert!(!is_rust_analyzer_wrapper(OsStr::new("sccache")));
    }
}
