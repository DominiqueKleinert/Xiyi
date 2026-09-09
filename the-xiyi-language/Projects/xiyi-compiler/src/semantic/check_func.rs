// src/semantic/check_func.rs
use std::mem;
use std::collections::HashMap;
use crate::ast::*;
use super::check_program::TypeChecker;

impl TypeChecker {
    // 原名 check_fn_def，按要求改名为 check_func。
    pub fn check_func(&mut self, fn_def: &FnDef) -> Result<(), String> {
        if self.fn_stack.contains(&fn_def.name) {
            return Err("error[MD002]: recursion not allowed in model block; graph must be topologically sortable".to_string());
        }
        self.fn_stack.push(fn_def.name.clone());

        self.scopes.push(HashMap::new());
        for param in &fn_def.params {
            let ty = if param.name == "self" {
                self.current_self_type.clone().unwrap_or(Type::SelfType)
            } else {
                param.ty.clone()
            };
            self.scopes.last_mut().unwrap().insert(param.name.clone(), ty);
        }

        // 保存旧值再设置新值，支持嵌套函数检查时不互相污染。
        // 注意：不能直接用 ? 提前返回——那样一旦 check_block_with_expected
        // 报错，下面恢复旧值那行会被跳过，current_return_type 就一直脏
        // 着，污染后续的检查。用 match 显式处理两种结果，保证恢复动作
        // 无论成功失败都会执行。
        let prev_return_type = self.current_return_type.clone();
        self.current_return_type = fn_def.return_type.clone();

        let body_type_result = self.check_block_with_expected(&fn_def.body, fn_def.return_type.as_ref());

        self.current_return_type = prev_return_type;

        let body_type = body_type_result?;

        if let Some(expected) = &fn_def.return_type {
            if !self.types_equal(&body_type, expected) {
                return Err(format!("expected return type {:?}, got {:?}", expected, body_type));
            }
            if !self.types_equal_with_privacy(&body_type, expected) {
                return Err(format!("privacy label mismatch: expected {:?}, got {:?}", expected, body_type));
            }
        }

        // forward 函数带差分隐私参数时，必须同时收一个 &mut
        // TrainingContext——这条检查是 model 领域知识，实现挪到了
        // check_model.rs，这里只是调用一下。
        self.check_forward_dp_requirement(fn_def)?;

        self.scopes.pop();
        self.fn_stack.pop();
        Ok(())
    }

    pub fn check_closure(
        &mut self,
        closure_expr: &Expr,
        param_ty: &Type,
        expected_ret: Option<&Type>,
    ) -> Result<Type, String> {
        let (param_name, body) = match &closure_expr.kind {
            ExprKind::Closure { param, body } => (param, body),
            _ => return Err("Expected a closure".to_string()),
        };

        let old_scopes = mem::take(&mut self.scopes);
        self.scopes.push(HashMap::new());
        self.scopes.last_mut().unwrap().insert(param_name.clone(), param_ty.clone());

        let ret_ty = self.check_expr(body);

        self.scopes = old_scopes;

        let ret_ty = ret_ty?;
        if let Some(expected) = expected_ret {
            if !self.types_equal_with_privacy(&ret_ty, expected) {
                return Err(format!(
                    "Closure return type {:?} does not match expected {:?}",
                    ret_ty, expected
                ));
            }
        }
        Ok(ret_ty)
    }
}
