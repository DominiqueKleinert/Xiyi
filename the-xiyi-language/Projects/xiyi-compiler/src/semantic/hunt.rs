// src/semantic/hunt.rs
use crate::ast::*;
use super::check_program::TypeChecker;

impl TypeChecker {
    // ===== 符号查找：一个裸标识符（ExprKind::Ident）到底指的是什么 =====
    // 从 check_expr 的 `ExprKind::Ident(name) => { ... }` 分支搬过来。
    // 查找顺序：
    //   1. 特判 "self"——当前正在检查的函数/方法体内，`self` 的类型
    //      来自 current_self_type（model 块/implement 块进入函数体检查
    //      前设置好的），不是一个普通作用域变量。
    //   2. 从内到外挨个扫作用域栈（scopes.iter().rev()，最内层优先），
    //      命中最近声明的那个同名变量。
    //   3. 都没找到，退一步查全局常量表（consts）。
    //   4. 全部落空才报"未定义"。
    pub fn hunt_symbol(&self, name: &str) -> Result<Type, String> {
        if name == "self" {
            if let Some(ty) = &self.current_self_type {
                return Ok(ty.clone());
            }
        }
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Ok(ty.clone());
            }
        }
        if let Some(ty) = self.consts.get(name) {
            return Ok(ty.clone());
        }
        Err(format!("undefined variable or constant: {}", name))
    }
}
