//! The command line: arguments, and what each subcommand does.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::error::CliError;
use crate::format::{InFormat, OutFormat};
use crate::ops;
use crate::ports::{self, IdSeed};
use crate::script::Script;

/// `drafftink-cli`: drafft.ink without a window.
///
/// Everything runs through the same command registry as the app, so a script written here does the same in the
/// app, over MCP and in tests. Nothing is written unless you name an output file.
#[derive(Debug, Parser)]
#[command(name = "drafftink-cli", version)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a command script against a document.
    ///
    /// The script is JSON Lines, a JSON array of steps, one step, or a batch object; `-` reads it from standard
    /// input.
    Run(RunArgs),
    /// Convert a document from one format to another.
    ///
    /// Inputs: drafft.ink (current or first version), Excalidraw, Mermaid text, PNG with the scene embedded.
    /// Outputs: drafft.ink, Excalidraw, SVG, and PNG when this build has the `png` feature (drawn on the CPU, no GPU).
    Convert(ConvertArgs),
    /// Print the outline of a document: one line per shape and per arrow, in reading order.
    Outline(OutlineArgs),
    /// Print the summary of a document: counts, bounds, groups, frames.
    Inspect(InspectArgs),
    /// List the commands, or describe one with its argument schema, facts and example.
    Commands(CommandsArgs),
    /// Print the argument and output schemas of every command as one JSON table.
    ///
    /// `scripts/build-web.sh` writes it next to the web app, which fetches it instead of carrying the schema code in
    /// its wasm module.
    Schemas,
}

/// Arguments of `run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// The script file, or `-` for standard input.
    pub script: PathBuf,
    /// A document to start from. Default: a blank document.
    #[arg(long, value_name = "FILE")]
    pub doc: Option<PathBuf>,
    /// The format of `--doc`.
    #[arg(long, value_enum, default_value = "auto")]
    pub from: InFormat,
    /// Write the document here when the script has finished. Nothing is written when a step refused.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// The format of `--out`. Default: from its extension.
    #[arg(long, value_enum)]
    pub to: Option<OutFormat>,
    /// Run all steps as one transaction (one undo step, rolled back at the first refusal). Steps may be named with
    /// `"as"` and refer to each other as `"$name"`.
    #[arg(long)]
    pub batch: bool,
    /// With --batch: run everything, report the outputs, then roll everything back.
    #[arg(long, requires = "batch")]
    pub dry_run: bool,
    /// With --batch: the undo label.
    #[arg(long, requires = "batch", value_name = "TEXT")]
    pub label: Option<String>,
    /// Make element ids and hand-drawn jitter from this seed, so the same script writes the same file every time.
    /// Without it they are random.
    #[arg(long, value_name = "N")]
    pub seed: Option<u64>,
    /// Without --batch: go on after a step that refused (the exit code is still 1).
    #[arg(long, conflicts_with = "batch")]
    pub keep_going: bool,
}

/// Arguments of `convert`.
#[derive(Debug, Args)]
pub struct ConvertArgs {
    /// The file to read.
    pub input: PathBuf,
    /// The file to write.
    pub output: PathBuf,
    /// The format of the input. Default: from its extension and content.
    #[arg(long, value_enum, default_value = "auto")]
    pub from: InFormat,
    /// The format of the output. Default: from its extension.
    #[arg(long, value_enum)]
    pub to: Option<OutFormat>,
}

/// Arguments of `outline`.
#[derive(Debug, Args)]
pub struct OutlineArgs {
    /// The file to read.
    pub input: PathBuf,
    /// The format of the input.
    #[arg(long, value_enum, default_value = "auto")]
    pub from: InFormat,
    /// Print the structure as JSON instead of text.
    #[arg(long)]
    pub json: bool,
    /// Most nodes to list.
    #[arg(long, default_value_t = 500)]
    pub max_nodes: u32,
}

/// Arguments of `inspect`.
#[derive(Debug, Args)]
pub struct InspectArgs {
    /// The file to read.
    pub input: PathBuf,
    /// The format of the input.
    #[arg(long, value_enum, default_value = "auto")]
    pub from: InFormat,
}

/// Arguments of `commands`.
#[derive(Debug, Args)]
pub struct CommandsArgs {
    /// Describe this command: its schemas, facts, refusals and example, as JSON.
    pub id: Option<String>,
    /// Only this family (`element`, `style`, `diagram`, ...).
    #[arg(long, conflicts_with = "id")]
    pub family: Option<String>,
    /// Only commands whose id, title or summary contains this text.
    #[arg(long, conflicts_with = "id")]
    pub search: Option<String>,
    /// Print JSON with the argument and output schemas of every listed command.
    #[arg(long, conflicts_with = "id")]
    pub json: bool,
}

fn read_script(path: &Path) -> Result<String, CliError> {
    if path == Path::new("-") {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(CliError::Stdin)?;
        return Ok(text);
    }
    std::fs::read_to_string(path).map_err(|source| CliError::Read { path: path.to_owned(), source })
}

/// Prints the notes about what was not kept, to the error stream.
fn warn(err: &mut dyn Write, notes: &[String]) -> Result<(), CliError> {
    for n in notes {
        writeln!(err, "warning: {n}")?;
    }
    Ok(())
}

fn run(args: &RunArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    // Everything that can be wrong with the arguments is found before anything runs.
    let target = args.out.as_deref().map(|p| OutFormat::resolve(args.to, p).map(|f| (p, f))).transpose()?;
    if matches!(target, Some((_, OutFormat::Png))) && !ports::RENDERS_PNG {
        return Err(CliError::NoPng);
    }
    let script = Script::parse(&read_script(&args.script)?)?;
    let mut s = ports::session(args.seed.map_or(IdSeed::Random, IdSeed::Fixed));
    if let Some(doc) = &args.doc {
        warn(err, &ops::open(&mut s, doc, args.from)?)?;
    }
    if args.batch || matches!(script, Script::Batch(_)) {
        ops::run_batch(&mut s, &script.into_batch(args.label.as_deref(), args.dry_run), out)?;
    } else if let Script::Steps(steps) = &script {
        ops::run_steps(&mut s, steps, args.keep_going, out)?;
    }
    if let Some((path, format)) = target {
        warn(err, &ops::save(&mut s, path, format)?)?;
        writeln!(err, "wrote {}", path.display())?;
    }
    Ok(())
}

fn convert(args: &ConvertArgs, err: &mut dyn Write) -> Result<(), CliError> {
    let format = OutFormat::resolve(args.to, &args.output)?;
    if format == OutFormat::Png && !ports::RENDERS_PNG {
        return Err(CliError::NoPng);
    }
    let mut s = ports::session(IdSeed::Random);
    warn(err, &ops::open(&mut s, &args.input, args.from)?)?;
    warn(err, &ops::save(&mut s, &args.output, format)?)?;
    writeln!(err, "wrote {}", args.output.display())?;
    Ok(())
}

/// Runs the command line `cli`, writing results to `out` and notes to `err`.
pub fn execute(command: &Command, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    match command {
        Command::Run(a) => run(a, out, err),
        Command::Convert(a) => convert(a, err),
        Command::Outline(a) => {
            let mut s = ports::session(IdSeed::Random);
            warn(err, &ops::open(&mut s, &a.input, a.from)?)?;
            writeln!(out, "{}", ops::outline(&mut s, a.json, a.max_nodes)?.trim_end())?;
            Ok(())
        }
        Command::Inspect(a) => {
            let mut s = ports::session(IdSeed::Random);
            warn(err, &ops::open(&mut s, &a.input, a.from)?)?;
            writeln!(out, "{}", ops::inspect(&mut s)?)?;
            Ok(())
        }
        Command::Schemas => {
            writeln!(out, "{}", drafftink_engine::schemas::dump(&drafftink_engine::Registry::global()))?;
            Ok(())
        }
        Command::Commands(a) => {
            let mut s = ports::session(IdSeed::Random);
            let text = match &a.id {
                Some(id) => ops::describe(&mut s, id)?,
                None => ops::commands(&mut s, a.family.as_deref(), a.search.as_deref(), a.json)?,
            };
            write!(out, "{}", text.trim_end())?;
            writeln!(out)?;
            Ok(())
        }
    }
}

/// Runs `command` as a process would: results on standard output, notes and errors on standard error, which
/// starts with `program` (the name the user typed: `drafftink-cli`, or `drafftink` for its subcommands). The exit
/// code is 0 when everything ran and 1 when a step or a file failed. A mistake in the arguments is found by the
/// parser before this runs, and it exits with 2.
///
/// `drafftink-cli` and the subcommands of `drafftink` both call this, so they cannot differ.
pub fn run_command(command: &Command, program: &str) -> ExitCode {
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    match execute(command, &mut out, &mut err) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(err, "{program}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The entry point of the `drafftink-cli` binary.
pub fn main() -> ExitCode {
    run_command(&Cli::parse().command, "drafftink-cli")
}
