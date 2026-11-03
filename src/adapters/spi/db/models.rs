//! Database-facing records used by SQLx.
use crate::domain::Source;
pub(super) struct SourceRow(pub Source);
