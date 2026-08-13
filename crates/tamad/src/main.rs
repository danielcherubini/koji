use std::env;

use anyhow::Result;
use tracing::info;

mod server;

#[derive(Debug, Clone)]
struct CliArgs {
    addr: String,
    protocol: String,
}

impl CliArgs {
    fn from_args() -> Result<Self> {
        let mut addr = "0.0.0.0:50051".to_string();
        let mut protocol = "grpc".to_string();

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--addr" => {
                    addr = args.next().unwrap_or_else(|| "0.0.0.0:50051".to_string());
                }
                "--protocol" => {
                    protocol = args.next().unwrap_or_else(|| "grpc".to_string());
                }
                _ => {
                    eprintln!("Unknown argument: {}", arg);
                    std::process::exit(1);
                }
            }
        }

        Ok(Self { addr, protocol })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = CliArgs::from_args()?;
    info!(
        addr = %args.addr,
        protocol = %args.protocol,
        "Starting tamad daemon"
    );

    server::start(&args.addr, &args.protocol).await
}
