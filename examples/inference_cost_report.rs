//! Compare externally graded, completed task receipts without making API calls.
use std::io::{self, Read};

use anyhow::Result;
use rara::{InferenceExperimentSample, InferencePriceTable};
use serde::Deserialize;

#[derive(Deserialize)]
struct Input {
    prices: InferencePriceTable,
    samples: Vec<InferenceExperimentSample>,
}

fn main() -> Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let input: Input = serde_json::from_str(&input)?;
    let report = input.prices.compare_tasks(&input.samples);
    serde_json::to_writer_pretty(io::stdout(), &report)?;
    Ok(())
}
