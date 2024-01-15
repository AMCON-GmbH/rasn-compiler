//! The `rasn-compiler` library is a parser combinator that parses ASN1 specifications and outputs
//! encoding-rule-agnotic rust representations of the ASN1 data elements to be used with the `rasn` crate.
//! The compiler heavily relies on the great library [nom](https://docs.rs/nom/latest/nom/) for its basic parsers.
//!
//! ## Example
//!
//! In order to compile ASN1 in your build process, invoke the rasn compiler in your [`build.rs` build script](https://doc.rust-lang.org/cargo/reference/build-scripts.html).
//!
//! ```rust
//! // build.rs build script
//! use std::path::PathBuf;
//! use rasn_compiler::RasnCompiler;
//!
//! fn main() {
//!   // Initialize the compiler
//!   match RasnCompiler::new()
//!     // add a single ASN1 source file
//!     .add_asn_by_path(PathBuf::from("spec_1.asn"))
//!     // add several ASN1 source files
//!     .add_asn_sources_by_path(vec![
//!         PathBuf::from("spec_2.asn"),
//!         PathBuf::from("spec_3.asn"),
//!     ].iter())
//!     // set an output path for the generated rust code
//!     .set_output_path(PathBuf::from("./asn/generated.rs"))
//!     // you may also compile literal ASN1 snippets
//!     .add_asn_literal("My-test-integer ::= INTEGER (1..128)")
//!     .compile() {
//!     Ok(warnings /* Vec<Box<dyn Error>> */) => { /* handle compilation warnings */ }
//!     Err(error /* Box<dyn Error> */) => { /* handle unrecoverable compilation error */ }
//!   }
//! }
//! ```
mod generator;
pub(crate) mod intermediate;
mod parser;
mod validator;

use std::{
    collections::BTreeMap,
    env::{self},
    error::Error,
    fs::{self, read_to_string},
    io::{self, Write},
    path::PathBuf,
    process::{Command, Stdio},
    rc::Rc,
    vec,
};

use generator::{builder::generate_module, GeneratedModule};
use intermediate::ToplevelDeclaration;
use parser::asn_spec;
use validator::Validator;

#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

#[cfg(target_family = "wasm")]
#[wasm_bindgen(inspectable, getter_with_clone)]
pub struct Generated {
    pub rust: String,
    pub warnings: String,
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn compile(asn1: &str) -> Result<Generated, JsValue> {
    RasnCompiler::new()
        .add_asn_literal(asn1)
        .compile_to_string()
        .map(|(rust, warn)| Generated {
            rust: format_bindings(&rust).unwrap_or(rust),
            warnings: warn.into_iter().fold(String::new(), |mut acc, w| {
                acc += &w.to_string();
                acc += "\n";
                acc
            }),
        })
        .map_err(|e| JsValue::from(e.to_string()))
}

/// The rasn compiler
#[derive(Debug, PartialEq)]
pub struct RasnCompiler<S: RasnCompilerState> {
    state: S,
}

/// Typestate representing compiler with missing parameters
pub struct CompilerMissingParams;

impl Default for CompilerMissingParams {
    fn default() -> Self {
        Self
    }
}

/// Typestate representing compiler that is ready to compile
pub struct CompilerReady {
    sources: Vec<AsnSource>,
    output_path: PathBuf,
}

/// Typestate representing compiler that has the output path set, but is missing ASN1 sources
pub struct CompilerOutputSet {
    output_path: PathBuf,
}

/// Typestate representing compiler that knows about ASN1 sources, but doesn't have an output path set
pub struct CompilerSourcesSet {
    sources: Vec<AsnSource>,
}

/// State of the rasn compiler
pub trait RasnCompilerState {}
impl RasnCompilerState for CompilerReady {}
impl RasnCompilerState for CompilerOutputSet {}
impl RasnCompilerState for CompilerSourcesSet {}
impl RasnCompilerState for CompilerMissingParams {}

struct CompileResult {
    pub modules: Vec<GeneratedModule>,
    pub warnings: Vec<Box<dyn Error>>,
}

#[derive(Debug, PartialEq)]
enum AsnSource {
    Path(PathBuf),
    Literal(String),
}

impl Default for RasnCompiler<CompilerMissingParams> {
    fn default() -> Self {
        RasnCompiler::new()
    }
}

impl RasnCompiler<CompilerMissingParams> {
    /// Provides a Builder for building rasn compiler commands
    pub fn new() -> RasnCompiler<CompilerMissingParams> {
        RasnCompiler {
            state: CompilerMissingParams,
        }
    }

    /// Add an ASN1 source to the compile command by path
    /// * `path_to_source` - path to ASN1 file to include
    pub fn add_asn_by_path(
        self,
        path_to_source: impl Into<PathBuf>,
    ) -> RasnCompiler<CompilerSourcesSet> {
        RasnCompiler {
            state: CompilerSourcesSet {
                sources: vec![AsnSource::Path(path_to_source.into())],
            },
        }
    }

    /// Add several ASN1 sources by path to the compile command
    /// * `path_to_source` - iterator of paths to the ASN1 files to be included
    pub fn add_asn_sources_by_path(
        self,
        paths_to_sources: impl Iterator<Item = impl Into<PathBuf>>,
    ) -> RasnCompiler<CompilerSourcesSet> {
        RasnCompiler {
            state: CompilerSourcesSet {
                sources: paths_to_sources
                    .map(|p| AsnSource::Path(p.into()))
                    .collect(),
            },
        }
    }

    /// Add a literal ASN1 source to the compile command
    /// * `literal` - literal ASN1 statement to include
    /// ```rust
    /// # use rasn_compiler::RasnCompiler;
    /// RasnCompiler::new().add_asn_literal("My-test-integer ::= INTEGER (1..128)").compile_to_string();
    /// ```
    pub fn add_asn_literal(self, literal: impl Into<String>) -> RasnCompiler<CompilerSourcesSet> {
        RasnCompiler {
            state: CompilerSourcesSet {
                sources: vec![AsnSource::Literal(literal.into())],
            },
        }
    }

    /// Set the output path for the generated rust representation.
    /// * `output_path` - path to an output file or directory, if path indicates
    ///                   a directory, the output file is named `rasn_generated.rs`
    pub fn set_output_path(
        self,
        output_path: impl Into<PathBuf>,
    ) -> RasnCompiler<CompilerOutputSet> {
        let mut path: PathBuf = output_path.into();
        if path.is_dir() {
            path.set_file_name("rasn_generated.rs");
        }
        RasnCompiler {
            state: CompilerOutputSet { output_path: path },
        }
    }
}

impl RasnCompiler<CompilerOutputSet> {
    /// Add an ASN1 source to the compile command by path
    /// * `path_to_source` - path to ASN1 file to include
    pub fn add_asn_by_path(
        self,
        path_to_source: impl Into<PathBuf>,
    ) -> RasnCompiler<CompilerReady> {
        RasnCompiler {
            state: CompilerReady {
                sources: vec![AsnSource::Path(path_to_source.into())],
                output_path: self.state.output_path,
            },
        }
    }

    /// Add several ASN1 sources by path to the compile command
    /// * `path_to_source` - iterator of paths to the ASN1 files to be included
    pub fn add_asn_sources_by_path(
        self,
        paths_to_sources: impl Iterator<Item = impl Into<PathBuf>>,
    ) -> RasnCompiler<CompilerReady> {
        RasnCompiler {
            state: CompilerReady {
                sources: paths_to_sources
                    .map(|p| AsnSource::Path(p.into()))
                    .collect(),
                output_path: self.state.output_path,
            },
        }
    }

    /// Add a literal ASN1 source to the compile command
    /// * `literal` - literal ASN1 statement to include
    /// ```rust
    /// # use rasn_compiler::RasnCompiler;
    /// RasnCompiler::new().add_asn_literal("My-test-integer ::= INTEGER (1..128)").compile_to_string();
    /// ```
    pub fn add_asn_literal(self, literal: impl Into<String>) -> RasnCompiler<CompilerReady> {
        RasnCompiler {
            state: CompilerReady {
                sources: vec![AsnSource::Literal(literal.into())],
                output_path: self.state.output_path,
            },
        }
    }
}

impl RasnCompiler<CompilerSourcesSet> {
    /// Add an ASN1 source to the compile command by path
    /// * `path_to_source` - path to ASN1 file to include
    pub fn add_asn_by_path(
        self,
        path_to_source: impl Into<PathBuf>,
    ) -> RasnCompiler<CompilerSourcesSet> {
        let mut sources: Vec<AsnSource> = self.state.sources;
        sources.push(AsnSource::Path(path_to_source.into()));
        RasnCompiler {
            state: CompilerSourcesSet { sources },
        }
    }

    /// Add several ASN1 sources by path to the compile command
    /// * `path_to_source` - iterator of paths to the ASN1 files to be included
    pub fn add_asn_sources_by_path(
        self,
        paths_to_sources: impl Iterator<Item = impl Into<PathBuf>>,
    ) -> RasnCompiler<CompilerSourcesSet> {
        let mut sources: Vec<AsnSource> = self.state.sources;
        sources.extend(paths_to_sources.map(|p| AsnSource::Path(p.into())));
        RasnCompiler {
            state: CompilerSourcesSet { sources },
        }
    }

    /// Add a literal ASN1 source to the compile command
    /// * `literal` - literal ASN1 statement to include
    /// ```rust
    /// # use rasn_compiler::RasnCompiler;
    /// RasnCompiler::new().add_asn_literal("My-test-integer ::= INTEGER (1..128)").compile_to_string();
    /// ```
    pub fn add_asn_literal(self, literal: impl Into<String>) -> RasnCompiler<CompilerSourcesSet> {
        let mut sources: Vec<AsnSource> = self.state.sources;
        sources.push(AsnSource::Literal(literal.into()));
        RasnCompiler {
            state: CompilerSourcesSet { sources },
        }
    }

    /// Set the output path for the generated rust representation.
    /// * `output_path` - path to an output file or directory, if path points to
    ///                   a directory, the compiler will generate a file for every ASN.1 module.
    ///                   If the path points to a file, all modules will be written to that file.
    pub fn set_output_path(self, output_path: impl Into<PathBuf>) -> RasnCompiler<CompilerReady> {
        RasnCompiler {
            state: CompilerReady {
                sources: self.state.sources,
                output_path: output_path.into(),
            },
        }
    }

    /// Runs the rasn compiler command and returns stringified Rust.
    /// Returns a Result wrapping a compilation result:
    /// * _Ok_  - tuple containing the stringified Rust representation of the ASN1 spec as well as a vector of warnings raised during the compilation
    /// * _Err_ - Unrecoverable error, no rust representations were generated
    pub fn compile_to_string(self) -> Result<(String, Vec<Box<dyn Error>>), Box<dyn Error>> {
        internal_compile(&self).map(|res| {
            (
                res.modules.iter().fold(String::new(), |mut acc, m| {
                    acc += &m.generated;
                    acc
                }),
                res.warnings,
            )
        })
    }
}

impl RasnCompiler<CompilerReady> {
    /// Add an ASN1 source to the compile command by path
    /// * `path_to_source` - path to ASN1 file to include
    pub fn add_asn_by_path(
        self,
        path_to_source: impl Into<PathBuf>,
    ) -> RasnCompiler<CompilerReady> {
        let mut sources: Vec<AsnSource> = self.state.sources;
        sources.push(AsnSource::Path(path_to_source.into()));
        RasnCompiler {
            state: CompilerReady {
                output_path: self.state.output_path,
                sources,
            },
        }
    }

    /// Add several ASN1 sources by path to the compile command
    /// * `path_to_source` - iterator of paths to the ASN1 files to be included
    pub fn add_asn_sources_by_path(
        self,
        paths_to_sources: impl Iterator<Item = impl Into<PathBuf>>,
    ) -> RasnCompiler<CompilerReady> {
        let mut sources: Vec<AsnSource> = self.state.sources;
        sources.extend(paths_to_sources.map(|p| AsnSource::Path(p.into())));
        RasnCompiler {
            state: CompilerReady {
                sources,
                output_path: self.state.output_path,
            },
        }
    }

    /// Add a literal ASN1 source to the compile command
    /// * `literal` - literal ASN1 statement to include
    /// ```rust
    /// # use rasn_compiler::RasnCompiler;
    /// RasnCompiler::new().add_asn_literal("My-test-integer ::= INTEGER (1..128)").compile_to_string();
    /// ```
    pub fn add_asn_literal(self, literal: impl Into<String>) -> RasnCompiler<CompilerReady> {
        let mut sources: Vec<AsnSource> = self.state.sources;
        sources.push(AsnSource::Literal(literal.into()));
        RasnCompiler {
            state: CompilerReady {
                output_path: self.state.output_path,
                sources,
            },
        }
    }

    /// Runs the rasn compiler command and returns stringified Rust.
    /// Returns a Result wrapping a compilation result:
    /// * _Ok_  - tuple containing the stringified Rust representation of the ASN1 spec as well as a vector of warnings raised during the compilation
    /// * _Err_ - Unrecoverable error, no rust representations were generated
    pub fn compile_to_string(self) -> Result<(String, Vec<Box<dyn Error>>), Box<dyn Error>> {
        RasnCompiler {
            state: CompilerSourcesSet {
                sources: self.state.sources,
            },
        }
        .compile_to_string()
    }

    /// Runs the rasn compiler command.
    /// Returns a Result wrapping a compilation result:
    /// * _Ok_  - Vector of warnings raised during the compilation
    /// * _Err_ - Unrecoverable error, no rust representations were generated
    pub fn compile(self) -> Result<Vec<Box<dyn Error>>, Box<dyn Error>> {
        let result = internal_compile(&RasnCompiler {
            state: CompilerSourcesSet {
                sources: self.state.sources,
            },
        })?;

        let generated = result.modules.iter().fold(String::new(), |mut acc, m| {
            acc += &m.generated;
            acc
        });
        fs::write(
            self.state
                .output_path
                .is_dir()
                .then(|| self.state.output_path.join("generated.rs"))
                .unwrap_or(self.state.output_path),
            format_bindings(&generated).unwrap_or(generated),
        )?;

        Ok(result.warnings)
    }
}

fn internal_compile(
    rasn: &RasnCompiler<CompilerSourcesSet>,
) -> Result<CompileResult, Box<dyn Error>> {
    let mut generated_modules = vec![];
    let mut warnings = Vec::<Box<dyn Error>>::new();
    let mut modules: Vec<ToplevelDeclaration> = vec![];
    for src in &rasn.state.sources {
        let stringified_src = match src {
            AsnSource::Path(p) => read_to_string(p)?,
            AsnSource::Literal(l) => l.clone(),
        };
        modules.append(
            &mut asn_spec(&stringified_src)?
                .into_iter()
                .flat_map(|(header, tlds)| {
                    let header_ref = Rc::new(header);
                    tlds.into_iter().enumerate().map(move |(index, mut tld)| {
                        tld.apply_tagging_environment(&header_ref.tagging_environment);
                        tld.set_index(header_ref.clone(), index);
                        tld
                    })
                })
                .collect(),
        );
    }
    let (valid_items, mut validator_errors) = Validator::new(modules).validate()?;
    let modules = valid_items.into_iter().fold(
        BTreeMap::<String, Vec<ToplevelDeclaration>>::new(),
        |mut modules, tld| {
            let key = tld
                .get_index()
                .map_or(<_>::default(), |(module, _)| module.name.clone());
            match modules.entry(key) {
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert(vec![tld]);
                }
                std::collections::btree_map::Entry::Occupied(ref mut e) => e.get_mut().push(tld),
            }
            modules
        },
    );
    for (_, module) in modules {
        let (rust_module, mut generator_errors) = generate_module(module)?;
        if let Some(m) = rust_module {
            generated_modules.push(m);
        }
        warnings.append(&mut generator_errors);
    }
    warnings.append(&mut validator_errors);

    Ok(CompileResult {
        modules: generated_modules,
        warnings,
    })
}

fn format_bindings(bindings: &String) -> Result<String, Box<dyn Error>> {
    let mut rustfmt = PathBuf::from(env::var("CARGO_HOME")?);
    rustfmt.push("bin/rustfmt");
    let mut cmd = Command::new(&*rustfmt);

    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());

    let mut child = cmd.spawn()?;
    let mut child_stdin = child.stdin.take().unwrap();
    let mut child_stdout = child.stdout.take().unwrap();

    // Write to stdin in a new thread, so that we can read from stdout on this
    // thread. This keeps the child from blocking on writing to its stdout which
    // might block us from writing to its stdin.
    let bindings = bindings.to_owned();
    let stdin_handle = ::std::thread::spawn(move || {
        let _ = child_stdin.write_all(bindings.as_bytes());
        bindings
    });

    let mut output = vec![];
    io::copy(&mut child_stdout, &mut output)?;

    let status = child.wait()?;
    let bindings = stdin_handle.join().expect(
        "The thread writing to rustfmt's stdin doesn't do \
             anything that could panic",
    );

    match String::from_utf8(output) {
        Ok(bindings) => match status.code() {
            Some(0) => Ok(bindings),
            Some(2) => Err(Box::new(io::Error::new(
                io::ErrorKind::Other,
                "Rustfmt parsing errors.".to_string(),
            ))),
            Some(3) => Ok(bindings),
            _ => Err(Box::new(io::Error::new(
                io::ErrorKind::Other,
                "Internal rustfmt error".to_string(),
            ))),
        },
        _ => Ok(bindings),
    }
}
