use std::env;
use std::error::Error;
use std::process::Command;

mod audio;
mod fluid;
mod fx;
mod synth;

fn main() -> Result<(), Box<dyn Error>> {
    match parse_args(env::args().skip(1))? {
        CliCommand::Run => fluid::run(),
        CliCommand::Update => update_nooise(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliCommand {
    Run,
    Update,
}

fn parse_args<I>(mut args: I) -> Result<CliCommand, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    let Some(arg) = args.next() else {
        return Ok(CliCommand::Run);
    };

    if args.next().is_some() {
        return Err(format!("unexpected argument after {arg}").into());
    }

    match arg.as_str() {
        "-h" | "--help" => {
            print_usage();
            std::process::exit(0);
        }
        "update" | "upgrade" => Ok(CliCommand::Update),
        other => Err(format!("unknown argument: {other}").into()),
    }
}

fn print_usage() {
    println!("Usage: nooise [update|upgrade]");
}

fn update_nooise() -> Result<(), Box<dyn Error>> {
    println!("Updating nooise from crates.io...");
    let status = Command::new("cargo")
        .args(["install", "nooise", "--locked", "--force"])
        .status()
        .map_err(|e| format!("failed to run cargo install nooise: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo install nooise failed with {status}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::{CliCommand, parse_args};

    fn args(items: &[&str]) -> impl Iterator<Item = String> {
        items
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn no_args_runs_app() {
        assert_eq!(parse_args(args(&[])).unwrap(), CliCommand::Run);
    }

    #[test]
    fn update_and_upgrade_run_updater() {
        assert_eq!(parse_args(args(&["update"])).unwrap(), CliCommand::Update);
        assert_eq!(parse_args(args(&["upgrade"])).unwrap(), CliCommand::Update);
    }

    #[test]
    fn unknown_arg_errors() {
        assert!(parse_args(args(&["--experiment"])).is_err());
    }
}
