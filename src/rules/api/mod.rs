//! The typed authoring surface a rule programs against (ADR-021): the query
//! primitives and verdict types ([`query`]), the sealed diagnostic type
//! ([`diagnostic`]), the witness vocabulary ([`witness`], ADR-019) and the
//! program-scoped derived-data cache ([`cache`]).
//!
//! This is the boundary the future external rule frontends bind to — nothing
//! in here reads the raw fixpoint structures beyond what the primitives
//! themselves certify.

pub mod cache;
pub mod diagnostic;
pub mod query;
pub mod witness;
