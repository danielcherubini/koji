#[macro_use]
mod sampling;

mod compaction;
mod general;
mod proxy;
mod supervisor;

pub(crate) use compaction::CompactionForm;
pub(crate) use general::GeneralForm;
pub(crate) use proxy::{ProxyAdvancedFields, ProxyBasicFields};
pub(crate) use sampling::SamplingForm;
pub(crate) use supervisor::SupervisorForm;
