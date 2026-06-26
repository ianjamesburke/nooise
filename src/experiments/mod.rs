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
pub(crate) mod t5_poc1;
pub(crate) mod t5_poc2;
pub(crate) mod t5_poc3;
pub(crate) mod t5_poc4;
pub(crate) mod t5_poc5;
pub(crate) mod t5_poc6;

pub(crate) fn run(experiment: &str) -> Result<(), Box<dyn Error>> {
    match experiment {
        "t1" => t1::run(),
        "t2" => t2::run(),
        "t3" => t3::run(),
        "t4" => t4::run(),
        "t5" | "t5a" => t5::run(t5::UiVariant::T5a),
        "t5b" => t5::run(t5::UiVariant::T5b),
        "t5c" => t5::run(t5::UiVariant::T5c),
        "t5d" => t5::run(t5::UiVariant::T5d),
        "t5e" => t5::run(t5::UiVariant::T5e),
        "t5_poc1" => t5_poc1::run().map_err(Into::into),
        "t5_poc2" => t5_poc2::run().map_err(Into::into),
        "t5_poc3" => t5_poc3::run().map_err(Into::into),
        "t5_poc4" => t5_poc4::run().map_err(Into::into),
        "t5_poc5" => t5_poc5::run().map_err(Into::into),
        "t5_poc6" => t5_poc6::run().map_err(Into::into),
        "r1" => r1::run(),
        "r2" => r2::run(),
        "r3" => r3::run(),
        "r4" => r4::run(),
        other => Err(format!("unknown experiment: {other}").into()),
    }
}
