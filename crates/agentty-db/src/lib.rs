//! Database connections for Agentty's database page and `agentty db`: which databases a project is
//! configured for ([`detect`]), which statements may run without asking ([`guard`]), and the
//! engines that run them.

pub mod detect;
pub mod engine;
pub mod guard;
pub mod model;
pub mod store;
