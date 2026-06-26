use std::env;
use std::error::Error;

mod audio;
mod fluid;
mod fx;
mod synth;

fn main() -> Result<(), Box<dyn Error>> {
    parse_args(env::args().skip(1))?;
    fluid::run()
}

fn parse_args<I>(mut args: I) -> Result<(), Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    if let Some(arg) = args.next() {
        if arg == "-h" || arg == "--help" {
            println!("Usage: nooise");
            std::process::exit(0);
        }
        return Err(format!("unknown argument: {arg}").into());
    }

    Ok(())
}
