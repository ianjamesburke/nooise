use clap::{Args, Parser, Subcommand};
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;
use update_check::check_for_update;

mod audio;
mod fluid;
mod fx;
mod synth;
mod update_check;

fn main() -> Result<(), Box<dyn Error>> {
    match Cli::parse().command {
        None => fluid::run(),
        Some(CliCommand::Song(args)) => run_song_code(args),
        Some(CliCommand::Update) => update_nooise(),
        Some(CliCommand::Render(args)) => render(args),
    }
}

#[derive(Debug, Parser)]
#[command(version, about, after_help = "Run a snapshot: nooise <CODE>")]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Clone, PartialEq, Subcommand)]
enum CliCommand {
    #[command(about = "Update nooise from crates.io", visible_alias = "upgrade")]
    Update,
    #[command(about = "Render the default mix to a wav file")]
    Render(RenderArgs),
    #[command(external_subcommand)]
    Song(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Args)]
struct RenderArgs {
    #[arg(long, default_value_t = 20.0)]
    seconds: f32,
    #[arg(long, default_value = "nooise.wav")]
    out: PathBuf,
    #[arg(long)]
    seed: Option<u64>,
}

fn render(args: RenderArgs) -> Result<(), Box<dyn Error>> {
    if args.seconds <= 0.0 {
        return Err("--seconds must be positive".into());
    }
    fluid::render_wav(args.seconds, &args.out, args.seed)
}

fn run_song_code(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let [code] = args.as_slice() else {
        return Err("expected exactly one song code".into());
    };
    let song = fluid::decode_song_code(code)?;
    fluid::run_with_song_state(song)
}

fn update_nooise() -> Result<(), Box<dyn Error>> {
    println!("Checking crates.io for nooise updates...");
    let Some(latest) = check_for_update()? else {
        println!("nooise is up to date (v{})", env!("CARGO_PKG_VERSION"));
        return Ok(());
    };

    let latest_version = latest.semver().to_string();
    println!("Updating nooise to {latest}...");
    let status = Command::new("cargo")
        .args(cargo_install_args(&latest_version))
        .status()
        .map_err(|e| format!("failed to run cargo install nooise: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo install nooise failed with {status}").into())
    }
}

fn cargo_install_args(version: &str) -> [&str; 6] {
    [
        "install",
        "nooise",
        "--locked",
        "--version",
        version,
        "--force",
    ]
}

#[cfg(test)]
mod tests {
    use super::{Cli, CliCommand, RenderArgs, cargo_install_args, render};
    use clap::{CommandFactory, Parser, error::ErrorKind};
    use std::path::PathBuf;

    fn parse(items: &[&str]) -> Result<Cli, clap::Error> {
        let args = std::iter::once("nooise").chain(items.iter().copied());
        Cli::try_parse_from(args)
    }

    #[test]
    fn no_args_runs_app() {
        assert_eq!(parse(&[]).unwrap().command, None);
    }

    #[test]
    fn song_code_arg_launches_app_with_snapshot() {
        let cli = parse(&["n1_abc"]).unwrap();

        assert_eq!(
            cli.command,
            Some(CliCommand::Song(vec!["n1_abc".to_string()]))
        );
    }

    #[test]
    fn version_flags_are_available() {
        assert_eq!(
            parse(&["-V"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
        assert_eq!(
            parse(&["--version"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn update_and_upgrade_run_updater() {
        assert_eq!(
            parse(&["update"]).unwrap().command,
            Some(CliCommand::Update)
        );
        assert_eq!(
            parse(&["upgrade"]).unwrap().command,
            Some(CliCommand::Update)
        );
    }

    #[test]
    fn updater_installs_exact_latest_version() {
        assert_eq!(
            cargo_install_args("1.2.3"),
            [
                "install",
                "nooise",
                "--locked",
                "--version",
                "1.2.3",
                "--force"
            ]
        );
    }

    #[test]
    fn unknown_arg_errors() {
        assert!(parse(&["--experiment"]).is_err());
    }

    #[test]
    fn render_defaults_and_flags_parse() {
        assert_eq!(
            parse(&["render"]).unwrap().command,
            Some(CliCommand::Render(RenderArgs {
                seconds: 20.0,
                out: PathBuf::from("nooise.wav"),
                seed: None,
            }))
        );
        assert_eq!(
            parse(&[
                "render",
                "--seconds",
                "3.5",
                "--out",
                "/tmp/x.wav",
                "--seed",
                "42"
            ])
            .unwrap()
            .command,
            Some(CliCommand::Render(RenderArgs {
                seconds: 3.5,
                out: PathBuf::from("/tmp/x.wav"),
                seed: Some(42),
            }))
        );
    }

    #[test]
    fn render_rejects_bad_input() {
        assert!(parse(&["render", "--seconds"]).is_err());
        assert!(parse(&["render", "--seconds", "abc"]).is_err());
        assert!(parse(&["render", "--loud"]).is_err());
        assert!(parse(&["update", "extra"]).is_err());
    }

    #[test]
    fn render_rejects_non_positive_seconds() {
        let err = render(RenderArgs {
            seconds: 0.0,
            out: PathBuf::from("/tmp/nooise-zero.wav"),
            seed: None,
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("--seconds must be positive"));
    }

    #[test]
    fn help_mentions_version_update_and_render() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("--version"));
        assert!(help.contains("update"));
        assert!(help.contains("upgrade"));
        assert!(help.contains("render"));
        assert!(help.contains("Run a snapshot: nooise <CODE>"));
    }
}
