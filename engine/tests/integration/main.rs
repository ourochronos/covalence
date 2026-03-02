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
//! | `test_split`       | `split`                                           |
//! | `test_merge`       | `merge`, `infer_edges`                            |
//! | `test_contention`  | `contention_check`, `resolve_contention`          |
//! | `test_decay`       | `decay_check`                                     |

mod helpers;

mod test_compile;
mod test_contention;
mod test_decay;
mod test_embed;
mod test_merge;
mod test_split;
mod test_tree;
