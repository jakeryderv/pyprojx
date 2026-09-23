use clap::Parser;

/// Version-aware intelligence for pyproject.toml.
///
/// pyprojx is a development stub: no commands are implemented yet.
#[derive(Debug, Parser)]
#[command(name = "pyprojx", version, about, long_about = None, arg_required_else_help = true)]
struct Cli {}

fn main() {
    Cli::parse();
}
