// src/semantic/helpers.rs
use std::fs;
use crate::ast::*;
use super::check_program::TypeChecker;

impl TypeChecker {
    pub fn resolve_import(&self, module_path: &str, stdlib_path: &str) -> Result<String, String> {
        let path = format!("{}/xiyi-core/src/{}.xiyi", stdlib_path, module_path);
        if fs::metadata(&path).is_ok() {
            Ok(path)
        } else {
            Err(format!("module not found: {}", module_path))
        }
    }

    pub fn is_compile_time_constant(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Literal(_) => true,
            ExprKind::Sym(_) => true,
            ExprKind::Ident(name) => self.consts.contains_key(name),
            ExprKind::BinaryOp { left, right, .. } => {
                self.is_compile_time_constant(left) && self.is_compile_time_constant(right)
            }
            // 一元负号也可能出现在常量表达式里（比如 -1 as ConstIntArray 元素）
            ExprKind::Unary { op: UnaryOp::Neg, expr } => self.is_compile_time_constant(expr),
            // lack &[T] 规范里明确写着"均为编译期常量"
            ExprKind::LackSlice(_) => true,
            _ => false,
        }
    }

    pub fn eval_const_int_expr(&self, expr: &Expr) -> Option<i64> {
        match &expr.kind {
            // 关键修复：原来这里只认 Literal::Int32，返回类型是 i64、
            // 参数是 i32，`Some(*v)` 直接类型不匹配（E0308）。而且这
            // 跟 check_expr.rs 那处是同一个病根——ast.rs 的 Literal
            // 有 20 个变体，只处理 Int32 意味着用户写 `3i64`/写一个大到
            // 被 parser 解析成 Int64/Int128 的字面量时，这里会静默查
            // 不到、返回 None，而不是报出"不是常量整数表达式"这种看得
            // 出病因的错误——extract_int_arg 会把这情况包装成一个说
            // 得过去的报错，但真实原因（这里根本没认出这个字面量）
            // 完全被掩盖了。现在把所有整数字面量变体都覆盖上，一律
            // `as i64` 转换（UInt64/UInt128/Usize 理论上可能超出 i64
            // 范围，但这是"用 i64 当通用常量整数类型"这个设计本身的
            // 取舍，不是这次要解决的问题，也没有比现状更差）。
            ExprKind::Literal(Literal::Int8(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::Int16(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::Int32(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::Int64(v)) => Some(*v),
            ExprKind::Literal(Literal::Int128(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::UInt8(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::UInt16(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::UInt32(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::UInt64(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::UInt128(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::Isize(v)) => Some(*v as i64),
            ExprKind::Literal(Literal::Usize(v)) => Some(*v as i64),
            ExprKind::Unary { op: UnaryOp::Neg, expr } => {
                self.eval_const_int_expr(expr).map(|v| -v)
            }
            ExprKind::BinaryOp { op, left, right } => {
                let l = self.eval_const_int_expr(left)?;
                let r = self.eval_const_int_expr(right)?;
                match op {
                    BinaryOp::Add => Some(l + r),
                    BinaryOp::Sub => Some(l - r),
                    BinaryOp::Mul => Some(l * r),
                    BinaryOp::Div => {
                        if r == 0 { None } else { Some(l / r) }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn shape_dim_to_i64(&self, dim: &ShapeDim) -> Result<i64, String> {
        match dim {
            ShapeDim::Const(c) => Ok(*c as i64),
            _ => Err("expected constant dimension".to_string()),
        }
    }

    pub fn get_call_arg_by_pos_or_name<'a>(
        &self,
        args: &'a [CallArg],
        pos: usize,
        name: &str,
    ) -> Result<(&'a Expr, String), String> {
        if let Some(arg) = args.get(pos) {
            match arg {
                CallArg::Positional(e) => return Ok((e, format!("positional #{}", pos + 1))),
                CallArg::Named(n, e) => {
                    if n == name {
                        return Ok((e, n.clone()));
                    }
                }
            }
        }

        for arg in args {
            if let CallArg::Named(n, e) = arg {
                if n == name {
                    return Ok((e, n.clone()));
                }
            }
        }

        Err(format!("argument '{}' not found (tried position {} and name '{}')", name, pos + 1, name))
    }

    pub fn extract_int_arg(&self, args: &[CallArg], name: &str) -> Result<i64, String> {
        for arg in args {
            if let CallArg::Named(n, expr) = arg {
                if n == name {
                    if let Some(v) = self.eval_const_int_expr(expr) {
                        return Ok(v);
                    } else {
                        return Err(format!(
                            "parameter '{}' is not a constant integer expression",
                            name
                        ));
                    }
                }
            }
        }
        Err(format!("parameter '{}' not found", name))
    }

    pub fn get_arg_type(&mut self, arg: &CallArg) -> Result<Type, String> {
        match arg {
            CallArg::Positional(expr) => self.check_expr(expr),
            CallArg::Named(_, expr) => self.check_expr(expr),
        }
    }

    pub fn check_call_arg(&mut self, arg: &CallArg) -> Result<Type, String> {
        match arg {
            CallArg::Positional(expr) => self.check_expr(expr),
            CallArg::Named(_, expr) => self.check_expr(expr),
        }
    }
}
