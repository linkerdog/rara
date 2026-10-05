//! Ordered background storage for the terminal session.

mod records;
mod worker;

pub(crate) use records::{RuntimeCheckpoint, WriteOperation};
pub(crate) use worker::ThreadIo;

#[cfg(test)]
mod tests;
