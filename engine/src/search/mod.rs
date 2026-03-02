pub mod dimension;
pub mod fusion;
pub mod graph;
pub mod lexical;
pub mod vector;

// Re-exported as public API surface; not all are consumed within this crate.
#[allow(unused_imports)]
pub use dimension::DimensionAdaptor;
#[allow(unused_imports)]
pub use fusion::ScoreFusion;
#[allow(unused_imports)]
pub use graph::GraphAdaptor;
#[allow(unused_imports)]
pub use lexical::LexicalAdaptor;
#[allow(unused_imports)]
pub use vector::VectorAdaptor;

// Re-export SearchMode so callers can use `covalence_engine::search::SearchMode`.
#[allow(unused_imports)]
pub use crate::services::search_service::SearchMode;
