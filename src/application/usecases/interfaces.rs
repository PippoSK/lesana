//! Execute one application operation while keeping its future Send.
use anyhow::Result;
use std::future::Future;
pub trait AbstractUseCase {
    type Output;
    fn execute(&self) -> impl Future<Output = Result<Self::Output>> + Send;
}
