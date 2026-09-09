// src/semantic/module.rs
mod check_program;
mod check_func;
mod check_stmt;
mod check_block;
mod check_expr;
mod check_type;
mod check_generic;
mod check_model;
mod helpers;
mod hunt;
mod lookup;
mod pattern;
mod privacy;
mod rational;

pub use check_program::TypeChecker;
pub use helpers::*;
pub use privacy::*;
pub use rational::*;
pub use check_type::*;
pub use check_generic::*;