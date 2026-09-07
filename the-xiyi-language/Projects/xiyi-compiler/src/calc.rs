// calc.rs
use crate::ast::{Literal, BinaryOp, UnaryOp};

pub struct Calc;

impl Calc {
    pub fn eval_binary_op(op: BinaryOp, left: Literal, right: Literal) -> Option<Literal> {
        match (left, right) {
            // ===== 有符号整数：Int8 =====
            (Literal::Int8(l), Literal::Int8(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::Int8),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::Int8),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::Int8),
                BinaryOp::Div => l.checked_div(r).map(Literal::Int8),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::Int8),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== Int16 =====
            (Literal::Int16(l), Literal::Int16(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::Int16),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::Int16),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::Int16),
                BinaryOp::Div => l.checked_div(r).map(Literal::Int16),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::Int16),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== Int32 =====
            (Literal::Int32(l), Literal::Int32(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::Int32),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::Int32),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::Int32),
                BinaryOp::Div => l.checked_div(r).map(Literal::Int32),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::Int32),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== Int64 =====
            (Literal::Int64(l), Literal::Int64(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::Int64),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::Int64),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::Int64),
                BinaryOp::Div => l.checked_div(r).map(Literal::Int64),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::Int64),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== Int128 =====
            (Literal::Int128(l), Literal::Int128(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::Int128),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::Int128),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::Int128),
                BinaryOp::Div => l.checked_div(r).map(Literal::Int128),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::Int128),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== 无符号整数：UInt8 =====
            (Literal::UInt8(l), Literal::UInt8(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::UInt8),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::UInt8),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::UInt8),
                BinaryOp::Div => l.checked_div(r).map(Literal::UInt8),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::UInt8),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== UInt16 =====
            (Literal::UInt16(l), Literal::UInt16(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::UInt16),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::UInt16),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::UInt16),
                BinaryOp::Div => l.checked_div(r).map(Literal::UInt16),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::UInt16),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== UInt32 =====
            (Literal::UInt32(l), Literal::UInt32(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::UInt32),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::UInt32),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::UInt32),
                BinaryOp::Div => l.checked_div(r).map(Literal::UInt32),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::UInt32),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== UInt64 =====
            (Literal::UInt64(l), Literal::UInt64(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::UInt64),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::UInt64),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::UInt64),
                BinaryOp::Div => l.checked_div(r).map(Literal::UInt64),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::UInt64),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== UInt128 =====
            (Literal::UInt128(l), Literal::UInt128(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::UInt128),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::UInt128),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::UInt128),
                BinaryOp::Div => l.checked_div(r).map(Literal::UInt128),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::UInt128),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== 平台相关整数：Isize =====
            (Literal::Isize(l), Literal::Isize(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::Isize),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::Isize),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::Isize),
                BinaryOp::Div => l.checked_div(r).map(Literal::Isize),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::Isize),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== Usize =====
            (Literal::Usize(l), Literal::Usize(r)) => match op {
                BinaryOp::Add => l.checked_add(r).map(Literal::Usize),
                BinaryOp::Sub => l.checked_sub(r).map(Literal::Usize),
                BinaryOp::Mul => l.checked_mul(r).map(Literal::Usize),
                BinaryOp::Div => l.checked_div(r).map(Literal::Usize),
                BinaryOp::Mod => l.checked_rem(r).map(Literal::Usize),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== 浮点数：Float16（内部用 f32 存储） =====
            (Literal::Float16(l), Literal::Float16(r)) => match op {
                BinaryOp::Add => Some(Literal::Float16(l + r)),
                BinaryOp::Sub => Some(Literal::Float16(l - r)),
                BinaryOp::Mul => Some(Literal::Float16(l * r)),
                BinaryOp::Div => Some(Literal::Float16(l / r)),
                // 浮点数没有取模运算
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== Float32 =====
            (Literal::Float32(l), Literal::Float32(r)) => match op {
                BinaryOp::Add => Some(Literal::Float32(l + r)),
                BinaryOp::Sub => Some(Literal::Float32(l - r)),
                BinaryOp::Mul => Some(Literal::Float32(l * r)),
                BinaryOp::Div => Some(Literal::Float32(l / r)),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== Float64 =====
            (Literal::Float64(l), Literal::Float64(r)) => match op {
                BinaryOp::Add => Some(Literal::Float64(l + r)),
                BinaryOp::Sub => Some(Literal::Float64(l - r)),
                BinaryOp::Mul => Some(Literal::Float64(l * r)),
                BinaryOp::Div => Some(Literal::Float64(l / r)),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                BinaryOp::Lt => Some(Literal::Bool(l < r)),
                BinaryOp::Gt => Some(Literal::Bool(l > r)),
                BinaryOp::Le => Some(Literal::Bool(l <= r)),
                BinaryOp::Ge => Some(Literal::Bool(l >= r)),
                _ => None,
            },

            // ===== 布尔值 =====
            (Literal::Bool(l), Literal::Bool(r)) => match op {
                BinaryOp::And => Some(Literal::Bool(l && r)),
                BinaryOp::Or => Some(Literal::Bool(l || r)),
                BinaryOp::Eq => Some(Literal::Bool(l == r)),
                BinaryOp::Neq => Some(Literal::Bool(l != r)),
                _ => None,
            },

            // ===== 类型不匹配或暂不支持的类型（Char/String/Unit/ByteString）=====
            _ => None,
        }
    }

    /// 一元运算常量折叠（! 和 -）
    pub fn eval_unary_op(op: UnaryOp, operand: Literal) -> Option<Literal> {
        match (op, operand) {
            // ---- 有符号整数取反 ----
            (UnaryOp::Neg, Literal::Int8(v)) => v.checked_neg().map(Literal::Int8),
            (UnaryOp::Neg, Literal::Int16(v)) => v.checked_neg().map(Literal::Int16),
            (UnaryOp::Neg, Literal::Int32(v)) => v.checked_neg().map(Literal::Int32),
            (UnaryOp::Neg, Literal::Int64(v)) => v.checked_neg().map(Literal::Int64),
            (UnaryOp::Neg, Literal::Int128(v)) => v.checked_neg().map(Literal::Int128),
            (UnaryOp::Neg, Literal::Isize(v)) => v.checked_neg().map(Literal::Isize),

            // ---- 浮点数取反 ----
            (UnaryOp::Neg, Literal::Float16(v)) => Some(Literal::Float16(-v)),
            (UnaryOp::Neg, Literal::Float32(v)) => Some(Literal::Float32(-v)),
            (UnaryOp::Neg, Literal::Float64(v)) => Some(Literal::Float64(-v)),

            // ---- 布尔取反 ----
            (UnaryOp::Not, Literal::Bool(v)) => Some(Literal::Bool(!v)),

            // ---- 不能对无符号整数取反 ----
            _ => None,
        }
    }
}