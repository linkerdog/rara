use anyhow::Result;
use rara_persistence::redaction::redact_secrets;

#[tokio::main]
#[expect(
    clippy::print_stderr,
    reason = "Final CLI error after terminal cleanup."
)]
async fn main() {
    if let Err(err) = main_impl().await {
        eprintln!("{}", redact_secrets(format!("Error: {err}")));
        std::process::exit(1);
    }
}

async fn main_impl() -> Result<()> {
    rara::run_cli().await
}
