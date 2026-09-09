// src/semantic/check_type.rs
use crate::ast::*;
use super::check_program::TypeChecker;

impl TypeChecker {
    // 看穿引用拿到底层类型（&str -> str，&&T -> T），方法调用时要用——
    // Rust 自己也是这样自动解引用去找方法的。
    pub fn strip_ref(&self, ty: &Type) -> Type {
        match ty {
            Type::Ref { inner, .. } => self.strip_ref(inner),
            other => other.clone(),
        }
    }

    // ===== 数值类型判断，Cast（as）、一元负号、算术运算都要用 =====
    // 关键修复：改回显式 match，不用 matches! 宏——项目里明确规定不用
    // Rust 的任何宏（包括 matches!），这几个函数之前是从没拆分的
    // sema.rs 里原样搬过来的，没有跟着改，这次一并清掉。
    pub fn is_numeric_type(&self, ty: &Type) -> bool {
        match ty {
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
            | Type::F16 | Type::F32 | Type::F64 => true,
            _ => false,
        }
    }

    // 一元负号只对"有符号"的数值类型有意义，U8..U128 排除在外
    // （对无符号数取负在这门语言里不打算隐式允许，想要的话得先 as 成有符号类型）
    pub fn is_signed_numeric_type(&self, ty: &Type) -> bool {
        match ty {
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::F16 | Type::F32 | Type::F64 => true,
            _ => false,
        }
    }

    // 索引表达式 expr[idx] 里的 idx 得是整数，不能是浮点数
    // （注：这门语言目前 Type 枚举里没有 usize/isize，vec.xiyi/string.xiyi
    // 里大量出现的 usize 现在其实解析不出来——这是另一个独立缺口，跟这次
    // 的 Index 表达式无关，等 parser 那边处理 usize 的时候一起补。这里先
    // 只要求"是整数类型"，不强求具体是哪一种）
    pub fn is_integer_type(&self, ty: &Type) -> bool {
        match ty {
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => true,
            _ => false,
        }
    }

    // 赋值语句的 target 从裸变量名换成任意表达式之后，得单独判断"这个
    // 表达式是不是一个能被赋值的位置"——变量、字段、索引可以，字面量、
    // 函数调用结果、二元运算结果这些都不行。目前允许的范围跟标准库
    // 实际用到的写法（i = ...、self.len = ...、arr[i] = ...）对齐，以后
    // 真要支持解引用赋值（*ptr = ...）再回来加 Unary 那个分支。
    pub fn is_assignable(&self, expr: &Expr) -> bool {
        match expr.kind {
            ExprKind::Ident(_) | ExprKind::FieldAccess { .. } | ExprKind::Index { .. } => true,
            _ => false,
        }
    }

    pub fn types_equal(&self, a: &Type, b: &Type) -> bool {
        match (a, b) {
            // ----- 整数类型 -----
            (Type::I8, Type::I8) => true,
            (Type::I16, Type::I16) => true,
            (Type::I32, Type::I32) => true,
            (Type::I64, Type::I64) => true,
            (Type::I128, Type::I128) => true,
            (Type::U8, Type::U8) => true,
            (Type::U16, Type::U16) => true,
            (Type::U32, Type::U32) => true,
            (Type::U64, Type::U64) => true,
            (Type::U128, Type::U128) => true,

            // ----- 浮点类型 -----
            (Type::F16, Type::F16) => true,
            (Type::F32, Type::F32) => true,
            (Type::F64, Type::F64) => true,

            // ----- 其他基本类型 -----
            (Type::Bool, Type::Bool) => true,
            (Type::Char, Type::Char) => true,
            (Type::Str, Type::Str) => true,
            (Type::Unit, Type::Unit) => true,

            // ----- 结构体、枚举、泛型等 -----
            (Type::Struct(name1), Type::Struct(name2)) => name1 == name2,
            (Type::Enum(name1), Type::Enum(name2)) => name1 == name2,
            (Type::Generic(name1, args1), Type::Generic(name2, args2)) => {
                if name1 != name2 {
                    return false;
                }
                if args1.len() != args2.len() {
                    return false;
                }
                args1.iter().zip(args2.iter()).all(|(a, b)| self.types_equal(a, b))
            }
            (Type::Ref { mutable: m1, inner: i1 }, Type::Ref { mutable: m2, inner: i2 }) => {
                m1 == m2 && self.types_equal(i1, i2)
            }
            // ===== 切片类型 [T]，递归比较元素类型 =====
            (Type::Slice(t1), Type::Slice(t2)) => self.types_equal(t1, t2),
            (
                Type::Tensor {
                    dtype: d1,
                    shape: s1,
                },
                Type::Tensor {
                    dtype: d2,
                    shape: s2,
                },
            ) => {
                if !self.types_equal(&**d1, &**d2) {
                    return false;
                }
                if s1.len() != s2.len() {
                    return false;
                }
                for (dim1, dim2) in s1.iter().zip(s2.iter()) {
                    match (dim1, dim2) {
                        (ShapeDim::Const(c1), ShapeDim::Const(c2)) => {
                            if c1 != c2 {
                                return false;
                            }
                        }
                        (ShapeDim::Sym(s1_name), ShapeDim::Sym(s2_name)) => {
                            if s1_name != s2_name {
                                return false;
                            }
                        }
                        (ShapeDim::Dyn, ShapeDim::Dyn) => {}
                        _ => return false,
                    }
                }
                true
            }
            (Type::ConstIntArray(v1), Type::ConstIntArray(v2)) => v1 == v2,
            (Type::Privacy(inner1, _), Type::Privacy(inner2, _)) => {
                self.types_equal(inner1, inner2)
            }
            (Type::Privacy(inner, _), other) => self.types_equal(inner, other),
            (other, Type::Privacy(inner, _)) => self.types_equal(other, inner),
            // ----- 泛型类型变量：只在"同一个变量名"时相等 -----
            // 注意：这里保持严格——types_equal 用在函数体内部（比如检查
            // `fn id<T>(x: T) -> T { x }` 的函数体返回值跟声明的返回类型
            // 是否一致），此时 T 应该被当成一个不透明的抽象类型，不能悄悄
            // 跟任何具体类型相等。真正"T 可以绑定成任意具体类型"这件事，
            // 只发生在调用点/构造点，交给 check_generic.rs 里的
            // unify_type，不要混进这里。
            (Type::Never, _) | (_, Type::Never) => true,
            (Type::TypeParam(n1), Type::TypeParam(n2)) => n1 == n2,
            _ => false,
        }
    }
}
