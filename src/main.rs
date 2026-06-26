use std::env;
use std::error::Error;

mod audio;
mod experiments;
mod fx;
mod sequencer;
mod synth;

fn main() -> Result<(), Box<dyn Error>> {
    let experiment = parse_experiment(env::args().skip(1))?;
    experiments::run(&experiment)
}

fn parse_experiment<I>(mut args: I) -> Result<String, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    let mut experiment = String::from("t1");

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--experiment" => {
                experiment = args
                    .next()
                    .ok_or("--experiment requires an experiment id")?;
            }
            "-h" | "--help" => {
                println!(
                    "Usage: nooise-engine --experiment <t1|t2|t3|t4|t5a|t5b|t5c|t5d|t5e|r1|r2|r3|r4>"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(experiment)
}
