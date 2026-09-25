//! Binary entry point: CLI parsing for `run`/`version`/`update`/`render`/`auto`
//! and a bare song code, then handoff to the matching `fluid` entry.

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
    let cli = Cli::parse();
    let bars = cli.bars.unwrap_or(fluid::DEFAULT_AUTO_BARS);
    match cli.command {
        None => match cli.song {
            None => fluid::run(),
            Some(song) => play_song(&song, bars),
        },
        Some(CliCommand::Update) => update_nooise(),
        Some(CliCommand::Render(args)) => render(args),
        Some(CliCommand::Auto) => fluid::run_auto(bars),
    }
}

#[derive(Debug, Parser)]
#[command(
    version,
    about,
    after_help = "Play a song: nooise <SONG>, where a song is a built-in number \
                  (nooise 9), several of them (nooise 9,10,11), or a shared code \
                  (nooise n1_...)."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
    /// A song to play: a built-in number (9), several of them (9,10,11), or a
    /// shared `n1_` code.
    song: Option<String>,
    /// Bars each song holds before morphing into the next. Defaults to 64.
    #[arg(long, global = true)]
    bars: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Subcommand)]
enum CliCommand {
    #[command(about = "Update nooise from crates.io", visible_alias = "upgrade")]
    Update,
    #[command(about = "Render the default mix to a wav file")]
    Render(RenderArgs),
    #[command(about = "Morph through every built-in song, forever")]
    Auto,
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

/// A song is named either by a shared `n1_` code or by its number in the
/// built-in set — the same thing reached two ways, so they share one argument
/// rather than one being a flag and the other a positional. Several numbers
/// morph through in the order given and loop.
fn play_song(song: &str, bars: u32) -> Result<(), Box<dyn Error>> {
    if song.starts_with(fluid::CODE_PREFIX) {
        let state = fluid::decode_song_code(song).map_err(|error| error.to_string())?;
        return fluid::run_with_song_state(state);
    }
    let numbers = song
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<usize>()
                .map_err(|_| format!("{part:?} is neither a song number nor an n1_ code").into())
        })
        .collect::<Result<Vec<usize>, Box<dyn Error>>>()?;
    fluid::run_songs(&numbers, bars)
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

    /// A song is one argument whether it is named by code or by number, and
    /// it is a plain positional, so flags read the same on either side of it.
    #[test]
    fn a_song_is_one_positional_named_by_code_or_by_number() {
        for (args, song, bars) in [
            (vec!["n1_abc"], "n1_abc", None),
            (vec!["9"], "9", None),
            (vec!["9,10,11"], "9,10,11", None),
            (vec!["9", "--bars", "4"], "9", Some(4)),
            (vec!["--bars", "4", "9"], "9", Some(4)),
        ] {
            let cli = parse(&args).unwrap();
            assert_eq!(cli.song.as_deref(), Some(song), "{args:?}");
            assert_eq!(cli.bars, bars, "{args:?}");
            assert_eq!(cli.command, None, "{args:?}");
        }
    }

    /// A subcommand name still wins over the song positional.
    #[test]
    fn subcommands_are_not_mistaken_for_songs() {
        assert!(matches!(
            parse(&["render"]).unwrap().command,
            Some(CliCommand::Render(_))
        ));
        assert_eq!(parse(&["auto"]).unwrap().command, Some(CliCommand::Auto));
        assert_eq!(parse(&["render"]).unwrap().song, None);
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

    /// `auto` used to take bars positionally. It takes none now, so an old
    /// `nooise auto 4` fails loudly instead of quietly meaning song four.
    #[test]
    fn auto_takes_bars_only_as_a_flag() {
        assert_eq!(parse(&["auto"]).unwrap().bars, None);
        assert_eq!(parse(&["auto", "--bars", "32"]).unwrap().bars, Some(32));
        assert!(parse(&["auto", "4"]).is_err());
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
        // The footer teaches the one grammar the CLI has: a song is a number,
        // a list of them, or a code.
        assert!(help.contains("nooise 9"));
        assert!(help.contains("nooise 9,10,11"));
        assert!(help.contains("nooise n1_..."));
    }
}
