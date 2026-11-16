#![forbid(unsafe_code)]
//! `ekiben-io-s3`: Bento's `aws_s3` input and output against S3 or an S3-compatible store
//! (MinIO), SigV4-signed by hand and sent through ekiben-io-http's outbound guard.

pub mod client;
pub mod input;
pub mod output;
pub mod sign;

use ekiben_core::Registry;

pub fn register_all(registry: &Registry) {
    registry.register_input("aws_s3", |conf, _ctx| {
        Ok(Box::new(input::S3Input::from_config(conf)?))
    });
    registry.register_output("aws_s3", output::build);
}
