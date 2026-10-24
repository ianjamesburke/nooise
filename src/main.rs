//! Binary entry point: CLI parsing for `run`/`version`/`update`/`render`/`auto`
//! and a bare song code, then handoff to the matching `fluid` entry.

use clap::{Args, CommandFactory, Parser, Subcommand};
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use update_check::check_for_update;

mod audio;
mod fluid;
mod fx;
mod synth;
mod update_check;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    if let Err(error) = cli.validate_from().and_then(|()| cli.validate_output()) {
        error.exit();
    }
    if matches!(
        cli.command,
        Some(CliCommand::Update | CliCommand::Render(_))
    ) {
        if cli.start_muted {
            return Err("-M/--start-muted only applies to live playback".into());
        }
        if cli.osc_in.is_some() {
            return Err("--osc-in only applies to live playback".into());
        }
        if cli.link {
            return Err("--link only applies to live playback".into());
        }
    }
    if cli.link && !cfg!(feature = "link") {
        return Err(format!(
            "--link needs Ableton Link support, which this build left out; \
             reinstall with `{}` (needs CMake and libclang)",
            LINK_INSTALL
        )
        .into());
    }
    if cli.audio_devices {
        println!("System Default (omit --output-device)");
        for name in audio::output_device_names()? {
            println!("{name}");
        }
        return Ok(());
    }
    let options = cli.live_options();
    let bars = cli.bars.unwrap_or(fluid::DEFAULT_AUTO_BARS);
    match cli.command {
        None => match cli.song.as_deref() {
            None => match cli.from {
                Some(from) => fluid::run_auto(bars, from, cli.osc, options),
                None => fluid::run(cli.osc, options),
            },
            Some(song) => play_song(song, bars, cli.osc, options),
        },
        Some(CliCommand::Update) => update_nooise(),
        Some(CliCommand::Render(args)) => render(args),
        Some(CliCommand::Auto) => fluid::run_auto(bars, cli.from.unwrap_or(1), cli.osc, options),
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
    /// Select an output device by its exact name; otherwise follow the system default.
    #[arg(
        long,
        global = true,
        value_name = "NAME",
        conflicts_with = "audio_devices"
    )]
    output_device: Option<String>,
    /// List current output devices and exit without starting playback.
    #[arg(long, global = true, conflicts_with = "from")]
    audio_devices: bool,
    /// Approximate bars per song plus transition, rounded to whole phrases. Defaults to 64.
    #[arg(long, global = true)]
    bars: Option<u32>,
    /// Start the full built-in song loop at this song number, then wrap around.
    #[arg(long, global = true, value_name = "SONG")]
    from: Option<usize>,
    /// Start live playback with Master Level at 0%.
    #[arg(short = 'M', long, global = true)]
    start_muted: bool,
    /// Experimental: derive BPM from taps on the Apple Silicon MacBook chassis.
    /// This is opt-in, needs no Accessibility or Input Monitoring permission,
    /// and only works on Macs exposing the undocumented SPU accelerometer.
    #[arg(long, global = true)]
    chassis_tap: bool,
    /// Mirror live telemetry, voice notes, and UI state as OSC over UDP for an
    /// external visualizer or bridge (foorm, baton). Bare `--osc` targets
    /// 127.0.0.1:9000; give ADDR to send elsewhere.
    #[arg(
        long,
        global = true,
        value_name = "ADDR",
        num_args = 0..=1,
        default_missing_value = fluid::DEFAULT_OSC_TARGET,
        require_equals = true
    )]
    osc: Option<SocketAddr>,
    /// Listen for inbound OSC commands and voice notes over UDP. Bare `--osc-in`
    /// binds 127.0.0.1:9001; give ADDR to listen elsewhere.
    #[arg(
        long,
        global = true,
        value_name = "ADDR",
        num_args = 0..=1,
        default_missing_value = fluid::DEFAULT_OSC_LISTEN,
        require_equals = true
    )]
    osc_in: Option<SocketAddr>,
    /// Join the Ableton Link session on the local network: shared tempo,
    /// bar phase, and start/stop with Live and other Link apps. Needs a
    /// build with `--features link`.
    #[arg(long, global = true)]
    link: bool,
}

impl Cli {
    fn validate_from(&self) -> Result<(), clap::Error> {
        if self.from.is_some()
            && (self.song.is_some() || !matches!(self.command, None | Some(CliCommand::Auto)))
        {
            return Err(Self::command().error(
                clap::error::ErrorKind::ArgumentConflict,
                "--from starts the full song loop; use it alone or with auto, without a separate song selection",
            ));
        }
        if let Some(from) = self.from {
            fluid::auto_song_numbers(from).map_err(|error| {
                Self::command().error(clap::error::ErrorKind::ValueValidation, error.to_string())
            })?;
        }
        Ok(())
    }

    fn validate_output(&self) -> Result<(), clap::Error> {
        let message = if self.audio_devices && (self.song.is_some() || self.command.is_some()) {
            Some("--audio-devices lists outputs; use it without a song or subcommand")
        } else if self.output_device.is_some()
            && matches!(
                self.command,
                Some(CliCommand::Render(_) | CliCommand::Update)
            )
        {
            Some("--output-device only applies to live playback")
        } else {
            None
        };
        match message {
            Some(message) => {
                Err(Self::command().error(clap::error::ErrorKind::ArgumentConflict, message))
            }
            None => Ok(()),
        }
    }

    fn live_options(&self) -> fluid::LiveStartOptions {
        fluid::LiveStartOptions {
            output: self.output_device.clone().map_or(
                audio::OutputSelection::SystemDefault,
                audio::OutputSelection::Device,
            ),
            start_muted: self.start_muted,
            chassis_tap: self.chassis_tap,
            osc_in: self.osc_in,
            #[cfg(feature = "link")]
            link: self.link,
        }
    }
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
fn play_song(
    song: &str,
    bars: u32,
    osc: Option<SocketAddr>,
    options: fluid::LiveStartOptions,
) -> Result<(), Box<dyn Error>> {
    if song.starts_with(fluid::CODE_PREFIX) {
        let state = fluid::decode_song_code(song).map_err(|error| error.to_string())?;
        return fluid::run_with_song_state(state, osc, options);
    }
    let numbers = song
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<usize>()
                .map_err(|_| format!("{part:?} is neither a song number nor an n1_ code").into())
        })
        .collect::<Result<Vec<usize>, Box<dyn Error>>>()?;
    fluid::run_songs(&numbers, bars, osc, options)
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

const LINK_INSTALL: &str = "cargo install nooise --locked --features link";

/// An update keeps the features this build has, so `nooise update` never
/// drops Link from a Link build.
fn cargo_install_args(version: &str) -> Vec<&str> {
    let mut args = vec![
        "install",
        "nooise",
        "--locked",
        "--version",
        version,
        "--force",
    ];
    if cfg!(feature = "link") {
        args.extend(["--features", "link"]);
    }
    args
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
    fn from_selects_the_auto_start_before_or_after_the_subcommand() {
        for args in [
            &["--from", "4"][..],
            &["auto", "--from", "4"],
            &["--from", "4", "auto"],
            &["auto", "--from", "4", "--bars", "8", "-M"],
        ] {
            let cli = parse(args).unwrap();
            cli.validate_from().unwrap();
            assert_eq!(cli.from, Some(4));
        }
        for args in [
            &["4", "--from", "2"][..],
            &["4,5", "--from", "2"],
            &["render", "--from", "2"],
            &["update", "--from", "2"],
        ] {
            assert_eq!(
                parse(args).unwrap().validate_from().unwrap_err().kind(),
                ErrorKind::ArgumentConflict
            );
        }
        for invalid in [0, crate::fluid::decode_auto_states().len() + 1, usize::MAX] {
            let number = invalid.to_string();
            assert_eq!(
                parse(&["--from", &number])
                    .unwrap()
                    .validate_from()
                    .unwrap_err()
                    .kind(),
                ErrorKind::ValueValidation
            );
        }
        for args in [&["--from"][..], &["--from", "nope"], &["--from", "-1"]] {
            assert!(parse(args).is_err());
        }
    }

    #[test]
    fn output_device_is_live_launch_configuration_and_listing_does_not_need_a_song() {
        for args in [
            &["--output-device", "BlackHole 2ch", "9"][..],
            &["9", "--output-device", "BlackHole 2ch"],
            &["auto", "--output-device", "BlackHole 2ch"],
        ] {
            assert_eq!(
                parse(args).unwrap().live_options().output,
                crate::audio::OutputSelection::Device("BlackHole 2ch".into())
            );
        }
        assert_eq!(
            parse(&[]).unwrap().live_options().output,
            crate::audio::OutputSelection::SystemDefault
        );
        assert!(parse(&["--audio-devices"]).unwrap().audio_devices);
        assert!(parse(&["--output-device"]).is_err());
        assert!(
            parse(&["render", "--output-device", "Speakers"])
                .unwrap()
                .validate_output()
                .is_err()
        );
        assert!(
            parse(&["--audio-devices", "9"])
                .unwrap()
                .validate_output()
                .is_err()
        );
        assert!(
            parse(&["auto", "--audio-devices"])
                .unwrap()
                .validate_output()
                .is_err()
        );
        assert!(parse(&["--audio-devices", "--output-device", "Speakers"]).is_err());
    }

    #[test]
    fn no_args_runs_app() {
        let cli = parse(&[]).unwrap();
        assert_eq!(cli.command, None);
        assert_eq!(cli.osc, None);
        assert_eq!(cli.osc_in, None);
        assert!(!cli.start_muted);
        assert!(!cli.link);
    }

    #[test]
    fn link_is_a_global_live_flag() {
        for args in [
            &["--link"][..],
            &["9", "--link"],
            &["--link", "9"],
            &["auto", "--link"],
        ] {
            assert!(parse(args).unwrap().link, "{args:?}");
        }
        assert_eq!(parse(&["--link", "9"]).unwrap().song.as_deref(), Some("9"));
    }

    #[test]
    fn start_muted_is_global_for_live_playback() {
        for args in [&["-M", "9"][..], &["9", "-M"], &["auto", "-M"]] {
            assert!(parse(args).unwrap().start_muted, "{args:?}");
        }
        assert!(!parse(&["9"]).unwrap().start_muted);
    }

    #[test]
    fn osc_in_defaults_to_listen_port_and_accepts_an_address() {
        let default = "127.0.0.1:9001".parse().ok();
        for args in [
            &["--osc-in"][..],
            &["9", "--osc-in"],
            &["--osc-in", "9"],
            &["auto", "--osc-in"],
        ] {
            let cli = parse(args).unwrap();
            assert_eq!(cli.osc_in, default, "{args:?}");
        }
        assert_eq!(
            parse(&["--osc-in=127.0.0.1:9100", "9"]).unwrap().osc_in,
            "127.0.0.1:9100".parse().ok()
        );
    }

    /// `--osc` is a global: it reads the same before a song, after it, and on
    /// `auto`. Bare `--osc` means foorm's default port; `--osc=ADDR` sends
    /// elsewhere; a value that is not a socket address is refused. The value
    /// needs `=` so a bare `--osc` never eats a following song argument.
    #[test]
    fn osc_target_defaults_to_foorm_and_accepts_an_address() {
        let default = "127.0.0.1:9000".parse().ok();
        for args in [
            &["--osc"][..],
            &["9", "--osc"],
            &["--osc", "9"],
            &["auto", "--osc"],
        ] {
            let cli = parse(args).unwrap();
            assert_eq!(cli.osc, default, "{args:?}");
        }
        assert_eq!(parse(&["--osc", "9"]).unwrap().song.as_deref(), Some("9"));
        assert_eq!(
            parse(&["--osc=10.0.0.5:7000", "9"]).unwrap().osc,
            "10.0.0.5:7000".parse().ok()
        );
        assert_eq!(
            parse(&["--osc=localhost"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
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
    fn updater_installs_exact_latest_version_with_this_builds_features() {
        let mut expected = vec![
            "install",
            "nooise",
            "--locked",
            "--version",
            "1.2.3",
            "--force",
        ];
        if cfg!(feature = "link") {
            expected.extend(["--features", "link"]);
        }
        assert_eq!(cargo_install_args("1.2.3"), expected);
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
