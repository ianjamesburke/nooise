use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;

mod audio;
mod fluid;
mod fx;
mod synth;

fn main() -> Result<(), Box<dyn Error>> {
    match parse_args(env::args().skip(1))? {
        CliCommand::Run => fluid::run(),
        CliCommand::Update => update_nooise(),
        CliCommand::Render(args) => fluid::render_wav(args.seconds, &args.out, args.seed),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum CliCommand {
    Run,
    Update,
    Render(RenderArgs),
}

#[derive(Debug, Clone, PartialEq)]
struct RenderArgs {
    seconds: f32,
    out: PathBuf,
    seed: Option<u64>,
}

fn parse_args<I>(mut args: I) -> Result<CliCommand, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    let Some(arg) = args.next() else {
        return Ok(CliCommand::Run);
    };

    match arg.as_str() {
        "-h" | "--help" => {
            print_usage();
            std::process::exit(0);
        }
        "update" | "upgrade" => {
            if let Some(extra) = args.next() {
                return Err(format!("unexpected argument after {arg}: {extra}").into());
            }
            Ok(CliCommand::Update)
        }
        "render" => parse_render_args(args),
        other => Err(format!("unknown argument: {other}").into()),
    }
}

fn parse_render_args<I>(mut args: I) -> Result<CliCommand, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    let mut render = RenderArgs {
        seconds: 20.0,
        out: PathBuf::from("nooise.wav"),
        seed: None,
    };

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--seconds" => {
                let v = args.next().ok_or("--seconds requires a value")?;
                render.seconds = v
                    .parse()
                    .map_err(|e| format!("invalid --seconds {v}: {e}"))?;
            }
            "--out" => {
                render.out = PathBuf::from(args.next().ok_or("--out requires a path")?);
            }
            "--seed" => {
                let v = args.next().ok_or("--seed requires a value")?;
                render.seed = Some(v.parse().map_err(|e| format!("invalid --seed {v}: {e}"))?);
            }
            other => return Err(format!("unknown render flag: {other}").into()),
        }
    }

    if render.seconds <= 0.0 {
        return Err("--seconds must be positive".into());
    }
    Ok(CliCommand::Render(render))
}

fn print_usage() {
    println!("Usage: nooise                                     run the TUI");
    println!("       nooise update|upgrade                      update from crates.io");
    println!("       nooise render [--seconds N] [--out PATH] [--seed N]");
    println!("                                                  render the default mix to a wav");
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
    use super::{CliCommand, RenderArgs, parse_args};
    use std::path::PathBuf;

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

    #[test]
    fn render_defaults_and_flags_parse() {
        assert_eq!(
            parse_args(args(&["render"])).unwrap(),
            CliCommand::Render(RenderArgs {
                seconds: 20.0,
                out: PathBuf::from("nooise.wav"),
                seed: None,
            })
        );
        assert_eq!(
            parse_args(args(&[
                "render",
                "--seconds",
                "3.5",
                "--out",
                "/tmp/x.wav",
                "--seed",
                "42"
            ]))
            .unwrap(),
            CliCommand::Render(RenderArgs {
                seconds: 3.5,
                out: PathBuf::from("/tmp/x.wav"),
                seed: Some(42),
            })
        );
    }

    #[test]
    fn render_rejects_bad_input() {
        assert!(parse_args(args(&["render", "--seconds"])).is_err());
        assert!(parse_args(args(&["render", "--seconds", "0"])).is_err());
        assert!(parse_args(args(&["render", "--seconds", "abc"])).is_err());
        assert!(parse_args(args(&["render", "--loud"])).is_err());
        assert!(parse_args(args(&["update", "extra"])).is_err());
    }
}
