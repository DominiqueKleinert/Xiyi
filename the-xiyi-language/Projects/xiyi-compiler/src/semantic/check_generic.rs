// src/semantic/check_generic.rs
use std::collections::HashMap;
use crate::ast::*;
use super::check_program::TypeChecker;

impl TypeChecker {
    // ===== 辅助：ast::GenericParam -> Vec<String>，跟 hir_builder.rs 里
    // 新加的 build_generic_params 是同一个用途，sema.rs 这边独立需要一份 =====
    pub fn generic_param_names(params: &[GenericParam]) -> Vec<String> {
        params
            .iter()
            .map(|gp| match gp {
                GenericParam::Type { name, .. } => name.clone(),
            })
            .collect()
    }

    // ===== 泛型实例化核心：合一（unify） =====
    //
    // expected 是声明里写的类型（可能包含 Type::TypeParam），actual 是调用点/
    // 构造点实际算出来的类型。bindings 记录这一次调用/构造过程中每个类型变量
    // 已经绑定成了什么，跨多个参数/字段共享——保证同一个类型变量在一次调用里
    // 前后绑定一致（比如 `fn pair<T>(a: T, b: T) -> T`，a、b 必须绑定成同一个
    // 具体类型，不能一个绑 i32 一个绑 bool）。
    //
    // 目前处理了"类型变量出现在最外层"和"嵌套在 Ref/Privacy/Tensor/Generic
    // 内部"两类情况的递归匹配，没有实现完整的高阶合一——现有测试用例（裸类型
    // 变量、或嵌套一层）都在这个范围内，遇到更复杂的场景再扩。
    pub fn unify_type(&self, actual: &Type, expected: &Type, bindings: &mut HashMap<String, Type>) -> bool {
        match expected {
            Type::TypeParam(name) => {
                if let Some(bound) = bindings.get(name) {
                    self.types_equal(bound, actual)
                } else {
                    bindings.insert(name.clone(), actual.clone());
                    true
                }
            }
            Type::Ref { mutable: em, inner: einner } => {
                if let Type::Ref { mutable: am, inner: ainner } = actual {
                    em == am && self.unify_type(ainner, einner, bindings)
                } else {
                    false
                }
            }
            // ===== 切片类型 [T]——跟 Ref 同一个套路，递归 unify 元素
            // 类型，这样 &[T] 里的 T 才能在调用点正确绑定，不是只靠
            // types_equal 死板比较（那样永远绑不出 T）。
            Type::Slice(einner) => {
                if let Type::Slice(ainner) = actual {
                    self.unify_type(ainner, einner, bindings)
                } else {
                    false
                }
            }
            Type::Privacy(einner, _) => {
                let stripped_actual = self.strip_privacy(actual);
                self.unify_type(&stripped_actual, einner, bindings)
            }
            Type::Generic(ename, eargs) => {
                if let Type::Generic(aname, aargs) = actual {
                    ename == aname
                        && eargs.len() == aargs.len()
                        && eargs.iter().zip(aargs.iter()).all(|(e, a)| self.unify_type(a, e, bindings))
                } else {
                    false
                }
            }
            Type::Tensor { dtype: edtype, shape: eshape } => {
                if let Type::Tensor { dtype: adtype, shape: ashape } = actual {
                    self.unify_type(adtype, edtype, bindings) && eshape == ashape
                } else {
                    false
                }
            }
            _ => self.types_equal(actual, expected),
        }
    }

    // 把 bindings 里记录的绑定代入 ty 中所有出现的 Type::TypeParam，结构性
    // 递归替换（Ref/Privacy/Tensor/Generic 内部也会替换）。没被绑定的类型
    // 变量原样保留成 TypeParam，不用假类型占位——调用点没能推导出来是真的
    // 推不出来，应该让后面用到它的地方去决定报错还是怎么处理，而不是悄悄
    // 塞一个 I32 掩盖过去。
    pub fn substitute_type(&self, ty: &Type, bindings: &HashMap<String, Type>) -> Type {
        match ty {
            Type::TypeParam(name) => bindings.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Type::Ref { mutable, inner } => Type::Ref {
                mutable: *mutable,
                inner: Box::new(self.substitute_type(inner, bindings)),
            },
            // 同 Ref，递归替换切片元素类型里的泛型参数
            Type::Slice(inner) => Type::Slice(Box::new(self.substitute_type(inner, bindings))),
            Type::Privacy(inner, tag) => {
                Type::Privacy(Box::new(self.substitute_type(inner, bindings)), tag.clone())
            }
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter().map(|a| self.substitute_type(a, bindings)).collect(),
            ),
            Type::Tensor { dtype, shape } => Type::Tensor {
                dtype: Box::new(self.substitute_type(dtype, bindings)),
                shape: shape.clone(),
            },
            other => other.clone(),
        }
    }
}
