//! Integration test suite for Covalence slow-path worker handlers (Issue #21).
//!
//! # Running
//!
//! ```bash
//! # Against the default dockerised DB (port 5434):
//! cargo test --test integration -- --test-threads=1
//!
//! # Against a custom DB:
//! DATABASE_URL=postgres://... cargo test --test integration -- --test-threads=1
//! ```
//!
//! Tests must run sequentially (`--test-threads=1`) because they share the
//! same Postgres instance.  Each test creates uniquely-identified rows and
//! removes them via a [`TestFixture`] RAII guard at the end.
//!
//! # Structure
//!
//! | Module             | Handlers covered                                  |
//! |--------------------|---------------------------------------------------|
//! | `test_embed`       | `embed`, `tree_embed`                             |
//! | `test_tree`        | `tree_index`                                      |
//! | `test_compile`     | `compile`                                         |
//! | `test_compilation` | smarter compilation — decision preservation (#35) |
//! | `test_split`       | `split`                                           |
//! | `test_merge`       | `merge`, `infer_edges`                            |
//! | `test_contention`  | `contention_check`, `resolve_contention`          |
//! | `test_decay`       | `decay_check`                                     |
//! | `test_distillation`| progressive distillation pipeline (#104)          |

mod helpers;

mod search_benchmark;
mod search_tests;
mod test_age_edge_sync;
mod test_auto_link;
mod test_cancellation;
mod test_compilation;
mod test_compile;
mod test_concerns;
mod test_consolidation;
mod test_content_hash;
mod test_contention;
mod test_critique;
mod test_dashboard;
mod test_decay;
mod test_distillation;
mod test_embed;
mod test_epistemic_slis;
mod test_ewc_structural;
mod test_faceted_classification;
mod test_gap_registry;
mod test_graph_health;
mod test_graph_stats;
mod test_kg_inference;
mod test_memory_recall;
mod test_merge;
mod test_namespace;
mod test_navigation;
mod test_occ;
mod test_openapi;
mod test_property;
mod test_reconsolidation;
mod test_search_explain;
mod test_search_freshness;
mod test_search_precision;
mod test_search_temporal;
mod test_security;
mod test_session_ingestion;
mod test_source_quality;
mod test_split;
mod test_spreading_activation;
mod test_staleness;
mod test_tasks;
mod test_temporal_edges;
mod test_tree;
mod test_whatif_retract;
