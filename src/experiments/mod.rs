use std::error::Error;

pub(crate) mod r1;
pub(crate) mod r2;
pub(crate) mod r3;
pub(crate) mod r4;
pub(crate) mod t1;
pub(crate) mod t2;
pub(crate) mod t3;
pub(crate) mod t4;
pub(crate) mod t5;

pub(crate) fn run(experiment: &str) -> Result<(), Box<dyn Error>> {
    match experiment {
        "t1" => t1::run(),
        "t2" => t2::run(),
        "t3" => t3::run(),
        "t4" => t4::run(),
        "t5" => t5::run(),
        "r1" => r1::run(),
        "r2" => r2::run(),
        "r3" => r3::run(),
        "r4" => r4::run(),
        other => Err(format!("unknown experiment: {other}").into()),
    }
}
