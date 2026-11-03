use crate::application::{
    contracts::{ResearchRequest, ResearchResult},
    usecases::retrieval::ToolFailure,
};
use anyhow::Result;
use std::{future::Future, pin::Pin};

pub trait ResearchTools: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: ResearchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ResearchResult, ToolFailure>> + Send + 'a>>;
}

pub enum ResearchOutcome {
    Complete,
    TurnLimit,
}
