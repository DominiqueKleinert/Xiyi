// src/semantic/check_expr.rs
use std::collections::HashMap;
use crate::ast::*;
use super::check_program::TypeChecker;

impl TypeChecker {
    pub fn check_binary_op(&self, op: &BinaryOp, left: &Type, right: &Type, left_expr: &Expr, right_expr: &Expr) -> Result<Type, String> {
        let left_inner = self.strip_privacy(left);
        let right_inner = self.strip_privacy(right);

        // 关键修复：裸整数字面量（0、1、2...）在 check_expr 里被固定标成
        // I32，但字面量本身没有"真实类型"——`while b != 0` 这种写法里的 0
        // 应该能跟 b 的 u128 兼容，不该反过来强制字面量也写成 u128。
        // 用"这一侧的原始表达式是不是字面量"来判断能不能放宽，而不是笼统
        // 放宽所有 I32——这样真正的 i32 变量跟 u128 变量比较时，还是会被
        // 正确地拦下来，不会被误放行。
        fn is_int_literal(e: &Expr) -> bool {
            match e.kind {
                ExprKind::Literal(Literal::Int32(_)) => true,
                _ => false,
            }
        }
        let left_is_literal = is_int_literal(left_expr);
        let right_is_literal = is_int_literal(right_expr);

        let both_numeric = self.is_numeric_type(&left_inner) && self.is_numeric_type(&right_inner);
        let types_compatible = self.types_equal(&left_inner, &right_inner)
            || (both_numeric && (left_is_literal || right_is_literal));

        // 结果类型：两边类型本来就一致就直接用；不一致但被字面量放宽了，
        // 就用"非字面量那侧"的真实类型（字面量给真类型让步）
        let result_ty = if self.types_equal(&left_inner, &right_inner) {
            left_inner.clone()
        } else if left_is_literal {
            right_inner.clone()
        } else {
            left_inner.clone()
        };

        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                // 关键修复：之前这里只认 I32/F32，rational.xiyi 里满篇的
                // i128/u128 算术（gcd 的取模、num/den 的乘加）全部会被
                // "invalid operands for arithmetic" 拦下来。改成认所有数值
                // 类型，两边类型仍然必须完全一致（不做隐式类型提升——
                // 混合类型算术得先显式 `as` 转换，跟 Rust 的严格风格一致），
                // 除非其中一边是裸字面量（见上面 types_compatible）。
                let is_scalar = both_numeric && types_compatible;
                let is_tensor = match (&left_inner, &right_inner) {
                    (Type::Tensor { .. }, Type::Tensor { .. }) => {
                        self.types_equal(&left_inner, &right_inner)
                    }
                    _ => false,
                };
                if is_scalar || is_tensor {
                    let tag = self.join_privacy_labels(left, right);
                    Ok(self.apply_privacy_tag(result_ty, tag))
                } else {
                    Err(format!(
                        "invalid operands for arithmetic: {:?} and {:?}",
                        left, right
                    ))
                }
            }
            BinaryOp::Eq
            | BinaryOp::Neq
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge => {
                if types_compatible {
                    let tag = self.join_privacy_labels(left, right);
                    Ok(self.apply_privacy_tag(Type::Bool, tag))
                } else {
                    Err(format!(
                        "comparison between different types: {:?} and {:?}",
                        left, right
                    ))
                }
            }
            BinaryOp::And | BinaryOp::Or => {
                if left_inner == Type::Bool && right_inner == Type::Bool {
                    let tag = self.join_privacy_labels(left, right);
                    Ok(self.apply_privacy_tag(Type::Bool, tag))
                } else {
                    Err("logical operators require bool operands".to_string())
                }
            }
        }
    }

    // ===== EnumVariantConstruction 的检查逻辑，独立成方法 =====
    //
    // 改用 unify_type/bindings 而不是死板的 types_equal，同时让无参变体
    // （比如 Option::None）在枚举本身有泛型参数时也一致返回 Type::Generic，
    // 而不是裸 Type::Enum（后者会跟同一个 match 里其他分支推出来的
    // Type::Generic 对不上，导致 match 分支类型不一致的假报错）。
    //
    // 新增的 `expected` 参数：如果调用方（目前只有 check_expr_with_expected）
    // 手头有一个外部期望类型（典型来源是显式类型标注，比如
    // `let _: Result<i32, i32> = Result::Ok(1);` 里的 `Result<i32, i32>`），
    // 就把它按声明顺序预置进 bindings——这样即使某个泛型参数完全没出现在
    // 这次构造的参数里（`Result<T, E>` 构造 `Ok(1)` 时，参数只能推出 T，
    // E 单靠参数永远推不出来），也能借助标注拿到值。这不是完整的双向类型
    // 推导，只处理"构造表达式外面刚好套了一层显式标注"这一种情况，够用。
    pub fn check_enum_variant_construction(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        args: &[CallArg],
        expected: Option<&Type>,
    ) -> Result<Type, String> {
        // 关键新增：`Rational::gcd(a, b)` 这种"限定路径调用"在 parser.rs 里
        // 跟 `Result::Ok(1)` 长得一模一样（都是 Ident::Ident(args)），parser
        // 没法只靠语法区分"枚举变体构造"和"调用某个类型 impl 块里的静态
        // 函数"，索性统一解析成 EnumVariantConstruction，把区分这件事留给
        // 这里——sema 手里有完整的符号表：先按枚举变体构造尝试，如果
        // enum_name 根本不是已知枚举，退一步查 self.methods（Item::Implement
        // 注册进去的函数表），当成限定路径的静态调用检查。两边都查不到
        // 才真正报错。
        if !self.enums.contains_key(enum_name) {
            if self.methods.get(enum_name).map_or(false, |m| m.contains_key(variant_name)) {
                return self.check_qualified_static_call(enum_name, variant_name, args, expected);
            }
            return Err(format!("undefined enum: {}", enum_name));
        }

        let enum_def = self.enums.get(enum_name)
            .ok_or_else(|| format!("undefined enum: {}", enum_name))?
            .clone();

        let variant = enum_def.variants.iter()
            .find(|v| v.name == *variant_name)
            .ok_or_else(|| format!("enum {} has no variant {}", enum_name, variant_name))?
            .clone();

        let mut bindings: HashMap<String, Type> = HashMap::new();

        // 关键新增：预置来自外部期望类型的绑定
        if let Some(Type::Generic(exp_name, exp_args)) = expected {
            if exp_name == enum_name && exp_args.len() == enum_def.generic_params.len() {
                let generic_names = Self::generic_param_names(&enum_def.generic_params);
                for (name, ty) in generic_names.iter().zip(exp_args.iter()) {
                    bindings.insert(name.clone(), ty.clone());
                }
            }
        }

        if let Some(expected_ty) = &variant.ty {
            if args.len() != 1 {
                return Err(format!("variant expects 1 argument"));
            }
            let arg_expr = match &args[0] {
                CallArg::Positional(e) => e,
                CallArg::Named(_, e) => e,
            };
            let arg_ty = self.check_call_arg(&args[0])?;
            // 关键修复：跟 check_struct_init 同一个坑——裸整数字面量默认是
            // I32，但变体payload 声明的可能是 i128/u64 这类别的数值类型
            // （目前已绑定的泛型参数也可能已经把 expected_ty 具体化成这类
            // 类型），字面量应该让步迁就真实类型。
            let resolved_expected = self.substitute_type(expected_ty, &bindings);
            let is_int_literal = match arg_expr.kind {
                ExprKind::Literal(Literal::Int32(_)) => true,
                _ => false,
            };
            let effective_arg_ty = if is_int_literal
                && self.is_numeric_type(&self.strip_privacy(&resolved_expected))
            {
                resolved_expected.clone()
            } else {
                arg_ty.clone()
            };
            // 注意：unify_type 遇到已经在 bindings 里的名字，会去校验一致性
            // 而不是直接覆盖——所以就算标注和实参同时提供了同一个类型变量的
            // 信息，这里仍然会检查两者是否矛盾，而不是标注说了算、实参不用管。
            if !self.unify_type(&effective_arg_ty, expected_ty, &mut bindings) {
                return Err(format!("type mismatch: expected {:?}, got {:?}", expected_ty, arg_ty));
            }
        } else if !args.is_empty() {
            return Err(format!("variant takes no arguments"));
        }

        if enum_def.generic_params.is_empty() {
            Ok(Type::Enum(enum_name.to_string()))
        } else {
            let generic_names = Self::generic_param_names(&enum_def.generic_params);
            let type_args: Vec<Type> = generic_names
                .iter()
                .map(|name| {
                    bindings
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| Type::TypeParam(name.clone()))
                })
                .collect();
            Ok(Type::Generic(enum_name.to_string(), type_args))
        }
    }

    // ===== StructInit 的检查逻辑，独立成方法，同样的 expected 提示套路 =====
    pub fn check_struct_init(
        &mut self,
        struct_name: &str,
        fields: &[(String, Expr)],
        expected: Option<&Type>,
    ) -> Result<Type, String> {
        let struct_def = self
            .structs
            .get(struct_name)
            .ok_or_else(|| format!("undefined struct: {}", struct_name))?
            .clone();
        if fields.len() != struct_def.fields.len() {
            return Err(format!(
                "struct {} expects {} fields, got {}",
                struct_name,
                struct_def.fields.len(),
                fields.len()
            ));
        }
        let mut field_map = HashMap::new();
        for field in &struct_def.fields {
            field_map.insert(field.name.clone(), field.ty.clone());
        }
        let mut bindings: HashMap<String, Type> = HashMap::new();

        if let Some(Type::Generic(exp_name, exp_args)) = expected {
            if exp_name == struct_name && exp_args.len() == struct_def.generic_params.len() {
                let generic_names = Self::generic_param_names(&struct_def.generic_params);
                for (name, ty) in generic_names.iter().zip(exp_args.iter()) {
                    bindings.insert(name.clone(), ty.clone());
                }
            }
        }

        for (field_name, field_expr) in fields {
            let expected_ty = field_map
                .get(field_name)
                .ok_or_else(|| format!("unknown field '{}' in struct {}", field_name, struct_name))?
                .clone();
            // 顺手用带期望类型的版本检查字段表达式——这样字段本身是
            // Ok(...)/Err(...)/嵌套 StructInit 时，也能像函数返回值那样
            // 受益于期望类型驱动的泛型参数推导（check_func 那边用的是
            // 同一套逻辑）。
            let actual_ty = self.check_expr_with_expected(field_expr, Some(&expected_ty))?;
            // 关键修复：裸整数字面量（0、1...）默认类型是 I32，但字段声明
            // 的是 i128/u64 这类别的数值类型时，字面量应该让步迁就字段的
            // 真实类型——跟 check_binary_op 里对字面量的处理是同一个道理，
            // 只是那次只覆盖了二元运算，没覆盖到结构体字段初始化这条独立
            // 路径。
            let is_int_literal = match field_expr.kind {
                ExprKind::Literal(Literal::Int32(_)) => true,
                _ => false,
            };
            let effective_ty = if is_int_literal
                && self.is_numeric_type(&self.strip_privacy(&expected_ty))
            {
                expected_ty.clone()
            } else {
                actual_ty.clone()
            };
            if !self.unify_type(&effective_ty, &expected_ty, &mut bindings) {
                return Err(format!(
                    "field '{}' type mismatch: expected {:?}, got {:?}",
                    field_name, expected_ty, actual_ty
                ));
            }
        }
        if struct_def.generic_params.is_empty() {
            Ok(Type::Struct(struct_name.to_string()))
        } else {
            let generic_names = Self::generic_param_names(&struct_def.generic_params);
            let type_args: Vec<Type> = generic_names
                .iter()
                .map(|name| {
                    bindings
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| Type::TypeParam(name.clone()))
                })
                .collect();
            Ok(Type::Generic(struct_name.to_string(), type_args))
        }
    }

    // ===== check_expr 的"带期望类型提示"版本 =====
    //
    // 目前只在 EnumVariantConstruction / StructInit 这两个会产生尚未绑定
    // 泛型参数的构造上使用这个提示；其他表达式种类原样委托给 check_expr，
    // 行为不变。调用方目前只有 Stmt::Let（当有显式类型标注时）。
    pub fn check_expr_with_expected(&mut self, expr: &Expr, expected: Option<&Type>) -> Result<Type, String> {
        match (&expr.kind, expected) {
            (ExprKind::EnumVariantConstruction { enum_name, variant_name, args }, Some(expected_ty)) => {
                self.check_enum_variant_construction(enum_name, variant_name, args, Some(expected_ty))
            }
            (ExprKind::StructInit { struct_name, fields }, Some(expected_ty)) => {
                self.check_struct_init(struct_name, fields, Some(expected_ty))
            }
            // 关键新增：裸 Ok(...)/Err(...)/Some(...)/None 这类写法（没有
            // Result::/Option:: 前缀）解析出来是 ExprKind::Call，不是
            // EnumVariantConstruction，之前这里漏了这条，导致 `Ok(...)`
            // 作为函数体最后一句、要跟声明的返回类型对齐推导泛型参数时，
            // 完全走不到这个"带期望类型"的分支，泛型参数（比如
            // Result<Rational, E> 里的 E）就留在没绑定的状态，跟声明的
            // 返回类型（比如 Result<Rational, ()>）对不上。
            (ExprKind::Call { qualifier: None, func, args, is_method: false }, Some(expected_ty)) => {
                let bare_matches: Vec<String> = self
                    .enums
                    .iter()
                    .filter(|(_, e)| e.variants.iter().any(|v| v.name == *func))
                    .map(|(name, _)| name.clone())
                    .collect();
                if bare_matches.len() == 1 {
                    self.check_enum_variant_construction(&bare_matches[0], func, args, Some(expected_ty))
                } else {
                    self.check_expr(expr)
                }
            }
            // 关键新增：裸整数字面量直接迁就期望类型。之前字面量的类型
            // 只在"直接出现在某个已知目标类型的位置"（结构体字段、二元
            // 运算的一侧、枚举变体参数）才会被特殊处理，`if` 分支这种
            // "字面量被 if 包了一层"的情况完全没人管，这里先把最基础的
            // 一层补上，供下面 If 分支递归调用时使用。
            (ExprKind::Literal(Literal::Int32(_)), Some(expected_ty))
                if self.is_numeric_type(&self.strip_privacy(expected_ty)) =>
            {
                Ok(expected_ty.clone())
            }
            // 关键新增：字面量的一元负号（比如 -1）同理——`-1` 现在解析成
            // Unary{Neg, Literal(1)}，不再是一个裸的 Literal(-1)，得单独
            // 认一下，不然 `let sign: i128 = if cond { -1 } else { 1 };`
            // 这种写法里的 -1 永远没法迁就 i128。
            (ExprKind::Unary { op: UnaryOp::Neg, expr: inner }, Some(expected_ty))
                if (match inner.kind {
                    ExprKind::Literal(Literal::Int32(_)) => true,
                    _ => false,
                }) && self.is_signed_numeric_type(&self.strip_privacy(expected_ty)) =>
            {
                Ok(expected_ty.clone())
            }
            // 关键新增：if 表达式——把期望类型递归传给 then/else 两个分支，
            // 这样 `let sign: i128 = if cond { -1 } else { 1 };` 这种写法，
            // 标注里的 i128 才能真正传到 -1/1 这两个字面量分支上，而不是
            // 各自独立按默认的 I32 检查、跟外层标注对不上。
            // 关键新增（补上上一轮漏掉的一层）：if 的 then/else 分支永远是
            // `{ ... }` 包起来的 Block，不是裸表达式——`{ -1 }` 实际上是
            // ExprKind::Block(Block{stmts:[ExprStmt(Unary{Neg,Literal(1)})]})。
            // 上一轮新增的 If 分支递归调用 check_expr_with_expected 时，
            // then_expr/else_expr 是 Block，根本匹配不到裸 Literal/Unary
            // 那两个分支，期望类型的传递在跨进 `{ }` 的那一刻就断了。这里
            // 补上 Block 分支，直接复用 check_block_with_expected（跟函数体
            // 那次用的是同一个函数），把期望类型继续往块里最后一句传。
            (ExprKind::Block(block), Some(expected_ty)) => {
                self.check_block_with_expected(block, Some(expected_ty))
            }
            (ExprKind::If { kind: if_kind, cond, then_expr, else_expr }, Some(expected_ty)) => {
                let cond_ty = self.check_expr(cond)?;
                if cond_ty != Type::Bool {
                    return Err("if condition must be bool".to_string());
                }
                let then_ty = self.check_expr_with_expected(then_expr, Some(expected_ty))?;
                match if_kind {
                    IfKind::Normal => {
                        let else_ty = match else_expr {
                            Some(e) => self.check_expr_with_expected(e, Some(expected_ty))?,
                            None => {
                                return Err(
                                    "if expression requires an else branch（如果这个 if 不需要产出值、纯粹是副作用，请显式写成 `lack if`）"
                                        .to_string(),
                                )
                            }
                        };
                        if !self.types_equal_with_privacy(&then_ty, &else_ty) {
                            return Err(format!(
                                "if branches have different types: then = {:?}, else = {:?}",
                                then_ty, else_ty
                            ));
                        }
                        let joined_tag = self.join_privacy_labels(&then_ty, &else_ty);
                        let base_ty = self.strip_privacy(&then_ty);
                        Ok(self.apply_privacy_tag(base_ty, joined_tag))
                    }
                    IfKind::Lack => {
                        if else_expr.is_some() {
                            return Err(
                                "`lack if` must not have an else branch（既然写了 else，就该用普通 if，不要用 lack if）"
                                    .to_string(),
                            );
                        }
                        let then_inner = self.strip_privacy(&then_ty);
                        if then_inner != Type::Unit {
                            return Err(format!(
                                "`lack if` 的 then 分支必须是 Unit 类型（纯副作用、不产出值），得到 {:?}",
                                then_ty
                            ));
                        }
                        Ok(Type::Unit)
                    }
                }
            }
            _ => self.check_expr(expr),
        }
    }

    // ==================== check_expr ====================
    pub fn check_expr(&mut self, expr: &Expr) -> Result<Type, String> {
        let result = match &expr.kind {
            // 关键修复：原来这里只覆盖了 Int32/Float32/Bool/String/
            // Unit/ByteString 六种——这是从 sema.rs 拆分过来时就带着的
            // 遗漏（拆分只是照原样搬运，没有替原文件补全过这里），
            // ast.rs 的 Literal 实际有 20 个变体，编译期 E0004 非穷尽
            // 匹配直接报了出来。现在全部覆盖，一个不落；Isize/Usize
            // 映射到 I64/U64——Type 枚举目前没有对应的 Isize/Usize
            // 变体（check_type.rs 的 is_integer_type 那边也提过这个
            // 缺口：vec.xiyi/string.xiyi 里大量出现的 usize 现在其实
            // 解析不出来），用平台常见的 64 位宽度做近似，等 Type 真
            // 补上 Isize/Usize 再改回来对应到位。
            ExprKind::Literal(lit) => match lit {
                Literal::Int8(_) => Ok(Type::I8),
                Literal::Int16(_) => Ok(Type::I16),
                Literal::Int32(_) => Ok(Type::I32),
                Literal::Int64(_) => Ok(Type::I64),
                Literal::Int128(_) => Ok(Type::I128),
                Literal::UInt8(_) => Ok(Type::U8),
                Literal::UInt16(_) => Ok(Type::U16),
                Literal::UInt32(_) => Ok(Type::U32),
                Literal::UInt64(_) => Ok(Type::U64),
                Literal::UInt128(_) => Ok(Type::U128),
                Literal::Isize(_) => Ok(Type::I64),
                Literal::Usize(_) => Ok(Type::U64),
                Literal::Float16(_) => Ok(Type::F16),
                Literal::Float32(_) => Ok(Type::F32),
                Literal::Float64(_) => Ok(Type::F64),
                Literal::Bool(_) => Ok(Type::Bool),
                Literal::Char(_) => Ok(Type::Char),
                Literal::String(_) => Ok(Type::Str),
                Literal::Unit => Ok(Type::Unit),
                // 字节字符串字面量，类型是 &[u8]
                Literal::ByteString(_) => Ok(Type::Ref {
                    mutable: false,
                    inner: Box::new(Type::Slice(Box::new(Type::U8))),
                }),
            },
            // 符号查找（self / 作用域 / 全局常量）挪到了 hunt.rs 的
            // hunt_symbol 里，这里只是委托调用。
            ExprKind::Ident(name) => self.hunt_symbol(name),
            ExprKind::Sym(_) => Ok(Type::I32),
            ExprKind::Closure { param, body } => {
                self.scopes.push(HashMap::new());
                self.scopes.last_mut().unwrap().insert(param.clone(), Type::F32);
                let body_ty = self.check_expr(body)?;
                self.scopes.pop();
                Ok(body_ty)
            }
            ExprKind::BinaryOp { op, left, right } => {
                let left_ty = self.check_expr(left)?;
                let right_ty = self.check_expr(right)?;
                self.check_binary_op(op, &left_ty, &right_ty, left, right)
            }
            // ===== 一元运算符（Neg / Not）=====
            ExprKind::Unary { op, expr } => {
                let inner_ty = self.check_expr(expr)?;
                let stripped = self.strip_privacy(&inner_ty);
                match op {
                    UnaryOp::Neg => {
                        if !self.is_signed_numeric_type(&stripped) {
                            return Err(format!(
                                "cannot apply unary `-` to {:?}（只支持有符号数值类型，\
                                无符号类型想取负得先 `as` 成有符号类型）",
                                inner_ty
                            ));
                        }
                        let privacy_tag = self.extract_privacy_tag(&inner_ty);
                        Ok(self.apply_privacy_tag(stripped, privacy_tag))
                    }
                    UnaryOp::Not => {
                        if stripped != Type::Bool {
                            return Err(format!(
                                "cannot apply `!` to non-bool type: {:?}",
                                inner_ty
                            ));
                        }
                        let privacy_tag = self.extract_privacy_tag(&inner_ty);
                        Ok(self.apply_privacy_tag(Type::Bool, privacy_tag))
                    }
                }
            }
            // ===== as 类型转换，目前只放开数值类型互转 =====
            ExprKind::Cast { expr, ty: cast_ty } => {
                let inner_ty = self.check_expr(expr)?;
                let stripped = self.strip_privacy(&inner_ty);
                if !self.is_numeric_type(&stripped) || !self.is_numeric_type(cast_ty) {
                    return Err(format!(
                        "invalid cast: `as` 目前只支持数值类型之间的转换，得到 {:?} as {:?}",
                        inner_ty, cast_ty
                    ));
                }
                let privacy_tag = self.extract_privacy_tag(&inner_ty);
                Ok(self.apply_privacy_tag(cast_ty.clone(), privacy_tag))
            }
            // ===== 索引表达式 expr[idx] =====
            ExprKind::Index { expr, index } => {
                let base_ty = self.check_expr(expr)?;
                let idx_ty = self.check_expr(index)?;
                let idx_stripped = self.strip_privacy(&idx_ty);
                if !self.is_integer_type(&idx_stripped) {
                    return Err(format!(
                        "index must be an integer type, got {:?}",
                        idx_ty
                    ));
                }

                // 剥掉引用/隐私标签，一路往里找"元素类型"：
                // - &T / &mut T -> 直接看里面的 T
                // - [T]（真正的切片类型，现在有了）-> T
                // - Vec<T>（或任何单参数泛型，兜底用）-> T
                // - Str -> 按字节索引，元素是 U8（对应 bytes[i] 这种写法）
                fn element_type(ty: &Type) -> Result<Type, String> {
                    match ty {
                        Type::Ref { inner, .. } => element_type(inner),
                        Type::Slice(inner) => Ok((**inner).clone()),
                        Type::Generic(_, args) if args.len() == 1 => Ok(args[0].clone()),
                        Type::Str => Ok(Type::U8),
                        other => Err(format!("type {:?} does not support indexing", other)),
                    }
                }
                let stripped_base = self.strip_privacy(&base_ty);
                let elem_ty = element_type(&stripped_base)?;
                let privacy_tag = self.extract_privacy_tag(&base_ty);
                Ok(self.apply_privacy_tag(elem_ty, privacy_tag))
            }
            // ===== lack &[T] 空切片字面量 =====
            ExprKind::LackSlice(elem_ty) => {
                // 关键修正：作者更新了规范（8.2.1）——泛型参数其实是【允许，
                // 单态化后验证】的，跟"必须是具体类型"这条旧规则正好相反。
                // 之前这里禁止 TypeParam 是按旧版规范写的，现在改成放开。
                //
                // 规范里真正该禁止的两类——impl Trait（error[LI009]）、
                // 未受约束的裸关联类型如 T::Item（error[LI008]）——这门
                // 语言的类型系统（ast::Type）里目前根本没有这两个概念
                // 对应的变体，语法层面写不出来，天然就不可能出现，所以
                // 这里没有对应的检查代码：不是漏检查，是压根不存在能触发
                // 它们的输入。等以后这门语言真的支持 impl Trait / 关联
                // 类型语法了，再回来把这两条错误码接上。
                //
                // never（warn[NE004]）同理：这门语言目前也没有 Type::Never
                // 这个类型，而且这套类型检查器目前只有"报错"这一种反馈
                // 机制（返回 Result<Type, String>），没有独立于报错之外的
                // "警告"通道——等 Never 类型和警告机制都补上了，再回来加
                // 这条 NE004。
                Ok(Type::Ref {
                    mutable: false,
                    inner: Box::new(Type::Slice(Box::new(elem_ty.clone()))),
                })
            }
            ExprKind::Call {
                qualifier,
                func,
                args,
                is_method,
            } => {
                // 关键新增：qualifier 非空说明这是 parser 侧未来可能产出的
                // 限定路径调用（目前 parser.rs 实际上还是把 `Type::func(...)`
                // 统一走 EnumVariantConstruction 那条路，check_enum_variant_construction
                // 里已经加了同样的兜底——这里加上是为了不管以后 parser 从哪条
                // 路产出 qualifier: Some(_)，sema 都认得，不用再改一遍。
                if let Some(q) = qualifier {
                    return self.check_qualified_static_call(q, func, args, None);
                }

                // 关键新增：裸 Ok(...)/Err(...)/Some(...)/None 这类写法——
                // 没有 Result::/Option:: 前缀，parser 只能把它们解析成普通
                // Call（qualifier: None），不是 EnumVariantConstruction。
                // 语言规范里这些写法就是不加前缀直接用的（等同于 Rust 里
                // Option::{Some,None}/Result::{Ok,Err} 被自动放进 prelude
                // 作用域）。这里不是专门为 Ok/Err/Some/None 硬编码四个名字，
                // 而是通用规则：在所有已注册的枚举里找"哪个枚举有一个恰好
                // 叫这个名字的变体"，找到且唯一就当枚举变体构造处理；同一个
                // 名字被多个枚举用作变体名时（真撞了）就报错让用户写限定
                // 路径消歧义，不去猜。
                if !is_method {
                    let matches: Vec<String> = self
                        .enums
                        .iter()
                        .filter(|(_, e)| e.variants.iter().any(|v| v.name == *func))
                        .map(|(name, _)| name.clone())
                        .collect();
                    if matches.len() == 1 {
                        return self.check_enum_variant_construction(&matches[0], func, args, None);
                    } else if matches.len() > 1 {
                        return Err(format!(
                            "ambiguous bare variant `{}`: matches multiple enums ({}), use a qualified path like EnumName::{}(...)",
                            func,
                            matches.join(", "),
                            func
                        ));
                    }
                }

                if func == "print" {
                    if self.in_model {
                        return Err(
                            "error[MD001]: side-effect not allowed in model block".to_string()
                        );
                    }
                    for arg in args {
                        self.check_call_arg(arg)?;
                    }
                    return Ok(Type::I32);
                }

                // `panic(msg)`——之前完全没注册过，sema.rs 查不到就报
                // "undefined function or method: panic"，codegen.rs 那边现在
                // 已经在处理 func == "panic" 时生成 Rust 的 panic!(...) 宏了。
                // 这里补类型检查：唯一参数必须是字符串。
                //
                // 返回类型比较特殊：panic 在运行时永远不会真的"返回"，这门
                // 语言的 Type 枚举里没有 Rust 那种 `!`（never）类型，不打算
                // 为这一个内置函数专门加一个新 Type 变体。这里先返回
                // Type::Unit，真正让它能出现在"其他分支返回 i32/bool/..."的
                // match 里的，是下面 ExprKind::Match 那段新加的豁免——分支
                // 表达式如果就是裸的 panic(...) 调用，不参与"所有分支类型
                // 必须一致"的比较（效果上等价于 Rust 的 `!` 能兼容任何类型）。
                if func == "panic" {
                    if args.len() != 1 {
                        return Err("`panic` expects exactly 1 argument (a message)".to_string());
                    }
                    let arg_ty = self.check_call_arg(&args[0])?;
                    if self.strip_privacy(&arg_ty) != Type::Str {
                        return Err(format!(
                            "`panic` expects a string argument, got {:?}",
                            arg_ty
                        ));
                    }
                    return Ok(Type::Never);    // 这里返回 Type::Never，表示 panic 永远不会返回
                }

                // `from_utf8_unchecked(bytes)`——同样是标准库里用了、但从没
                // 被定义过的内建函数（string.xiyi 的 as_str 方法用它把
                // &[u8] 强转成 &str），跟 panic 是同一类问题。参数必须是
                // &[u8]，返回 &str；不检查调用点是不是真的在 unsafe 块内——
                // 这门编译器目前没有追踪"当前是否处于 unsafe 上下文"的机制，
                // 属于另一个独立的、更大的安全检查缺口，不在这次范围内。
                if func == "from_utf8_unchecked" {
                    if args.len() != 1 {
                        return Err(
                            "`from_utf8_unchecked` expects exactly 1 argument".to_string()
                        );
                    }
                    let arg_ty = self.check_call_arg(&args[0])?;
                    let expected_arg_ty = Type::Ref {
                        mutable: false,
                        inner: Box::new(Type::Slice(Box::new(Type::U8))),
                    };
                    if !self.types_equal(&self.strip_privacy(&arg_ty), &expected_arg_ty) {
                        return Err(format!(
                            "`from_utf8_unchecked` expects &[u8], got {:?}",
                            arg_ty
                        ));
                    }
                    return Ok(Type::Ref {
                        mutable: false,
                        inner: Box::new(Type::Str),
                    });
                }

                if let Some(ty) = self.try_cross_model_call(func, args)? {
                    return Ok(ty);
                }

                if self.in_model && self.fn_stack.contains(func) {
                    return Err(
                        "error[MD002]: recursion not allowed in model block; graph must be topologically sortable"
                            .to_string(),
                    );
                }

                // ---- 普通函数调用（含泛型函数：fn id<T>(x: T) -> T） ----
                if !*is_method {
                    if let Some(fn_def) = self.functions.get(func) {
                        let fn_params = fn_def.params.clone();
                        let fn_return = fn_def.return_type.clone();

                        if fn_params.len() != args.len() {
                            return Err(format!(
                                "function `{}` expects {} arguments, got {}",
                                func, fn_params.len(), args.len()
                            ));
                        }

                        // 关键修复：以前这里用 types_equal 死板比较，`id(42)` 这种
                        // 调用会拿 I32 去跟声明里写的 T（Type::TypeParam("T")）比较，
                        // 永远不相等。现在改成 unify_type——遇到 T 就记录“T 绑定成了
                        // 什么”，同一个函数调用里所有参数共享同一张绑定表，保证
                        // `fn pair<T>(a: T, b: T)` 这种多处用到同一个 T 的场景绑定一致。
                        let mut bindings: HashMap<String, Type> = HashMap::new();
                        for (param, arg) in fn_params.iter().zip(args) {
                            let arg_ty = self.check_call_arg(arg)?;
                            if !self.unify_type(&arg_ty, &param.ty, &mut bindings) {
                                return Err(format!(
                                    "type mismatch in call to `{}`: parameter `{}` expected {:?}, got {:?}",
                                    func, param.name, param.ty, arg_ty
                                ));
                            }
                        }

                        // 返回类型里出现的 T 也要代入绑定结果，否则 `id(42)` 的返回类型
                        // 还是裸的 Type::TypeParam("T")，后面 `let _ = id(42);` 之类的赋值
                        // 检查又会因为类型对不上而报错。
                        // 关键修复：函数没声明返回类型时，之前兜底成
                        // I32——一个"没写返回类型"的函数语义上该是 Unit
                        // （不返回有意义的值），跟 I32 完全是两码事，硬编码
                        // 成 I32 会在这类函数的调用结果被用在别处时，跟
                        // 真实语义对不上。
                        let result_ty = fn_return
                            .map(|ret| self.substitute_type(&ret, &bindings))
                            .unwrap_or(Type::Unit);
                        return Ok(result_ty);
                    }
                }

                // ===== tensor.cond =====
                if func == "tensor.cond" {
                    let input_expr = self.get_call_arg_by_pos_or_name(args, 0, "input")?.0;
                    let cond_expr = self.get_call_arg_by_pos_or_name(args, 1, "condition")?.0;
                    let then_expr = self.get_call_arg_by_pos_or_name(args, 2, "then")?.0;
                    let else_expr = self.get_call_arg_by_pos_or_name(args, 3, "else")?.0;

                    let input_ty = self.check_expr(input_expr)?;
                    let _ = self.check_closure(cond_expr, &input_ty, Some(&Type::Bool))?;
                    let then_ty = self.check_closure(then_expr, &input_ty, Some(&input_ty))?;
                    let else_ty = self.check_closure(else_expr, &input_ty, Some(&input_ty))?;
                    if !self.types_equal_with_privacy(&then_ty, &else_ty) {
                        return Err("tensor.cond 'then' and 'else' branches have different types".to_string());
                    }
                    return Ok(input_ty);
                }

                // ===== tensor.while_loop =====
                if func == "tensor.while_loop" {
                    let init_expr = self.get_call_arg_by_pos_or_name(args, 0, "init")?.0;
                    let cond_expr = self.get_call_arg_by_pos_or_name(args, 1, "cond")?.0;
                    let body_expr = self.get_call_arg_by_pos_or_name(args, 2, "body")?.0;

                    let init_ty = self.check_expr(init_expr)?;
                    let _ = self.check_closure(cond_expr, &init_ty, Some(&Type::Bool))?;
                    let _ = self.check_closure(body_expr, &init_ty, Some(&init_ty))?;
                    return Ok(init_ty);
                }

                if func == "embedding" {
                    if args.len() < 2 {
                        return Err("embedding requires at least two arguments".to_string());
                    }
                    let receiver_ty = self.get_arg_type(&args[0])?;
                    let _num_embeddings = self.extract_int_arg(args, "num_embeddings")?;
                    let embedding_dim = self.extract_int_arg(args, "embedding_dim")?;
                    if let Type::Tensor { dtype, ref shape } = self.strip_privacy(&receiver_ty) {
                        let mut new_shape = shape.clone();
                        new_shape.push(ShapeDim::Const(embedding_dim as usize));
                        let privacy_tag = self.extract_privacy_tag(&receiver_ty);
                        let result_ty = Type::Tensor {
                            dtype: Box::new(Type::F32),
                            shape: new_shape,
                        };
                        return Ok(self.apply_privacy_tag(result_ty, privacy_tag));
                    } else {
                        return Err(format!("embedding expects a tensor, got {:?}", receiver_ty));
                    }
                }

                if func == "linear" {
                    if args.len() < 2 {
                        return Err("linear requires at least two arguments".to_string());
                    }
                    let receiver_ty = self.get_arg_type(&args[0])?;
                    let in_val = self.extract_int_arg(args, "in")?;
                    let out_val = self.extract_int_arg(args, "out")?;
                    if let Type::Tensor { dtype, ref shape } = self.strip_privacy(&receiver_ty) {
                        if let Some(last) = shape.last() {
                            if let ShapeDim::Const(c) = last {
                                if *c != in_val as usize {
                                    return Err(format!(
                                        "shape mismatch in linear: input last dim {} does not match 'in' value {}",
                                        c, in_val
                                    ));
                                }
                            }
                            let mut new_shape = shape.clone();
                            if let Some(last) = new_shape.last_mut() {
                                *last = ShapeDim::Const(out_val as usize);
                            } else {
                                return Err("tensor must have at least one dimension".to_string());
                            }
                            let privacy_tag = self.extract_privacy_tag(&receiver_ty);
                            let result_ty = Type::Tensor { dtype, shape: new_shape };
                            return Ok(self.apply_privacy_tag(result_ty, privacy_tag));
                        } else {
                            return Err("tensor must have at least one dimension for linear".to_string());
                        }
                    } else {
                        return Err(format!("linear expects a tensor, got {:?}", receiver_ty));
                    }
                }

                if func == "conv2d" {
                    if args.len() < 2 {
                        return Err("conv2d requires at least 2 arguments".to_string());
                    }
                    let receiver_ty = self.get_arg_type(&args[0])?;
                    let stripped = self.strip_privacy(&receiver_ty);
                    if let Type::Tensor { dtype, ref shape } = stripped {
                        if shape.len() != 3 && shape.len() != 4 {
                            return Err("conv2d expects 3D [C, H, W] or 4D [B, C, H, W] tensor".to_string());
                        }
                        let (c_idx, h_idx, w_idx) = if shape.len() == 4 { (1, 2, 3) } else { (0, 1, 2) };

                        let _in_channels = self.extract_int_arg(args, "in")?;
                        let out_channels = self.extract_int_arg(args, "out")?;
                        let kernel = self.extract_int_arg(args, "kernel")?;
                        let stride = self.extract_int_arg(args, "stride").unwrap_or(1);
                        let padding = self.extract_int_arg(args, "padding").unwrap_or(0);

                        let h_out = match &shape[h_idx] {
                            ShapeDim::Const(h) => {
                                let h = *h as i64;
                                let h_out = (h + 2 * padding - kernel) / stride + 1;
                                if h_out <= 0 {
                                    return Err("conv2d output height non-positive".to_string());
                                }
                                ShapeDim::Const(h_out as usize)
                            }
                            _ => ShapeDim::Dyn,
                        };
                        let w_out = match &shape[w_idx] {
                            ShapeDim::Const(w) => {
                                let w = *w as i64;
                                let w_out = (w + 2 * padding - kernel) / stride + 1;
                                if w_out <= 0 {
                                    return Err("conv2d output width non-positive".to_string());
                                }
                                ShapeDim::Const(w_out as usize)
                            }
                            _ => ShapeDim::Dyn,
                        };

                        let mut new_shape = shape.clone();
                        new_shape[c_idx] = ShapeDim::Const(out_channels as usize);
                        new_shape[h_idx] = h_out;
                        new_shape[w_idx] = w_out;

                        let privacy_tag = self.extract_privacy_tag(&receiver_ty);
                        let result_ty = Type::Tensor { dtype, shape: new_shape };
                        return Ok(self.apply_privacy_tag(result_ty, privacy_tag));
                    } else {
                        return Err(format!("conv2d expects a tensor, got {:?}", receiver_ty));
                    }
                }

                if func == "max_pool2d" {
                    if args.len() < 1 {
                        return Err("max_pool2d requires at least 1 argument".to_string());
                    }
                    let receiver_ty = self.get_arg_type(&args[0])?;
                    let kernel = self.extract_int_arg(args, "kernel")?;
                    let stride = self.extract_int_arg(args, "stride").unwrap_or(kernel);
                    if let Type::Tensor { dtype, ref shape } = self.strip_privacy(&receiver_ty) {
                        if shape.len() != 3 && shape.len() != 4 {
                            return Err("max_pool2d expects 3D or 4D tensor".to_string());
                        }
                        let (h_idx, w_idx) = if shape.len() == 4 { (2, 3) } else { (1, 2) };

                        let h_out = match &shape[h_idx] {
                            ShapeDim::Const(h) => {
                                let h = *h as i64;
                                let h_out = (h - kernel) / stride + 1;
                                if h_out <= 0 {
                                    return Err("max_pool2d output height non-positive".to_string());
                                }
                                ShapeDim::Const(h_out as usize)
                            }
                            _ => ShapeDim::Dyn,
                        };
                        let w_out = match &shape[w_idx] {
                            ShapeDim::Const(w) => {
                                let w = *w as i64;
                                let w_out = (w - kernel) / stride + 1;
                                if w_out <= 0 {
                                    return Err("max_pool2d output width non-positive".to_string());
                                }
                                ShapeDim::Const(w_out as usize)
                            }
                            _ => ShapeDim::Dyn,
                        };
                        let mut new_shape = shape.clone();
                        new_shape[h_idx] = h_out;
                        new_shape[w_idx] = w_out;
                        let privacy_tag = self.extract_privacy_tag(&receiver_ty);
                        let result_ty = Type::Tensor { dtype, shape: new_shape };
                        return Ok(self.apply_privacy_tag(result_ty, privacy_tag));
                    } else {
                        return Err(format!("max_pool2d expects a tensor, got {:?}", receiver_ty));
                    }
                }

                if func == "flatten" {
                    if args.len() < 1 {
                        return Err("flatten requires at least 1 argument".to_string());
                    }
                    let receiver_ty = self.get_arg_type(&args[0])?;
                    if let Type::Tensor { dtype, ref shape } = self.strip_privacy(&receiver_ty) {
                        let mut total = 1;
                        let mut all_const = true;
                        for dim in shape {
                            if let ShapeDim::Const(c) = dim {
                                total *= *c;
                            } else {
                                all_const = false;
                                break;
                            }
                        }
                        let privacy_tag = self.extract_privacy_tag(&receiver_ty);
                        let result_ty = if all_const {
                            let new_shape = vec![ShapeDim::Const(total)];
                            Type::Tensor { dtype, shape: new_shape }
                        } else {
                            Type::Tensor {
                                dtype: dtype.clone(),
                                shape: shape.clone(),
                            }
                        };
                        return Ok(self.apply_privacy_tag(result_ty, privacy_tag));
                    } else {
                        return Err(format!("flatten expects a tensor, got {:?}", receiver_ty));
                    }
                }

                if func == "reshape" {
                    if args.len() < 2 {
                        return Err("reshape requires target shape".to_string());
                    }
                    let receiver_ty = self.get_arg_type(&args[0])?;
                    let shape_vals = match &args[1] {
                        CallArg::Positional(expr) | CallArg::Named(_, expr) => {
                            let arg_ty = self.check_expr(expr)?;
                            if let Type::ConstIntArray(vals) = arg_ty {
                                vals
                            } else {
                                return Err(
                                    "reshape expects a constant integer array for shape".to_string()
                                );
                            }
                        }
                    };
                    let new_shape: Vec<ShapeDim> = shape_vals
                        .iter()
                        .map(|&v| ShapeDim::Const(v as usize))
                        .collect();
                    let dtype = match self.strip_privacy(&receiver_ty) {
                        Type::Tensor { dtype, .. } => dtype,
                        _ => return Err("reshape expects a tensor".to_string()),
                    };
                    let privacy_tag = self.extract_privacy_tag(&receiver_ty);
                    let result_ty = Type::Tensor { dtype, shape: new_shape };
                    return Ok(self.apply_privacy_tag(result_ty, privacy_tag));
                }

                if func == "relu" || func == "dropout" || func == "layer_norm" {
                    if args.len() < 1 {
                        return Err(format!("{} requires at least 1 argument", func));
                    }
                    let arg_ty = self.get_arg_type(&args[0])?;
                    return Ok(arg_ty);
                }

                if func == "sum" {
                    if args.len() < 1 {
                        return Err("sum requires at least 1 argument".to_string());
                    }
                    let _ = self.get_arg_type(&args[0])?;
                    return Ok(Type::F32);
                }

                // 方法调用：查方法表（builtin 内建方法表 + self.methods 里
                // implement 块登记的方法），按方法自己的签名（含泛型）检查。
                // 逻辑挪到了 lookup.rs 的 check_method_call 里，这里只是
                // 委托调用。
                if *is_method {
                    return self.check_method_call(func, args);
                }

                if self.in_model && self.model_names.contains(func) {
                    if let Some(ty) = self.try_cross_model_call(func, args)? {
                        return Ok(ty);
                    }
                }

                for arg in args {
                    self.check_call_arg(arg)?;
                }
                Err(format!("undefined function or method: {}", func))
            }
            ExprKind::Block(block) => self.check_block(block),
            ExprKind::StructInit { struct_name, fields } => {
                self.check_struct_init(struct_name, fields, None)
            }
            ExprKind::FieldAccess { struct_expr, field_name } => {
                let struct_ty = self.check_expr(struct_expr)?;
                let stripped_ty = self.strip_privacy(&struct_ty);
                match stripped_ty {
                    Type::Struct(name) => {
                        let struct_def = self
                            .structs
                            .get(&name)
                            .ok_or_else(|| format!("undefined struct: {}", name))?;
                        for field in &struct_def.fields {
                            if field.name == *field_name {
                                return Ok(self.strip_privacy(&field.ty));
                            }
                        }
                        Err(format!("field '{}' not found in struct {}", field_name, name))
                    }
                    // ===== 泛型结构体（比如 Vec<T>）=====
                    // current_self_type 在泛型 implement 块里存的是
                    // Generic("Vec", [TypeParam("T")])，不是 Struct("Vec")——
                    // Vec 自己的方法体访问 self.len/self.cap/self.ptr 这些
                    // 字段时，一直被前面那个 Struct 分支漏掉，落进最下面的
                    // "field access on non-struct type" 兜底错误。这里按
                    // 结构体名找到定义，同时用泛型实参（这里是 [TypeParam("T")]，
                    // 还没具体化）替换字段声明类型里的泛型参数——
                    // 保证以后要是有字段类型直接引用 T（比如 ptr: *mut T），
                    // 取出来的类型也是正确替换过的，不是裸的 TypeParam。
                    Type::Generic(name, type_args) => {
                        let struct_def = self
                            .structs
                            .get(&name)
                            .cloned()
                            .ok_or_else(|| format!("undefined struct: {}", name))?;
                        let param_names: Vec<String> = struct_def
                            .generic_params
                            .iter()
                            .map(|gp| match gp {
                                GenericParam::Type { name, .. } => name.clone(),
                            })
                            .collect();
                        let bindings: HashMap<String, Type> = param_names
                            .into_iter()
                            .zip(type_args.into_iter())
                            .collect();
                        for field in &struct_def.fields {
                            if field.name == *field_name {
                                let field_ty = self.substitute_type(&field.ty, &bindings);
                                return Ok(self.strip_privacy(&field_ty));
                            }
                        }
                        Err(format!("field '{}' not found in struct {}", field_name, name))
                    }
                    Type::SelfType => {
                        if let Some(ty) = &self.current_self_type {
                            if let Type::Struct(name) = ty {
                                let struct_def = self
                                    .structs
                                    .get(name)
                                    .ok_or_else(|| format!("undefined struct: {}", name))?;
                                for field in &struct_def.fields {
                                    if field.name == *field_name {
                                        return Ok(self.strip_privacy(&field.ty));
                                    }
                                }
                                return Err(format!(
                                    "field '{}' not found in struct {}",
                                    field_name, name
                                ));
                            }
                        }
                        Err("field access on SelfType with no current self type".to_string())
                    }
                    other => Err(format!("field access on non-struct type: {:?}", other)),
                }
            }
            ExprKind::Range { start, end } => {
                let start_ty = self.check_expr(start)?;
                let end_ty = self.check_expr(end)?;
                if (start_ty == Type::I32 || start_ty == Type::I64)
                    && (end_ty == Type::I32 || end_ty == Type::I64)
                {
                    Ok(Type::I32)
                } else {
                    Err("range bounds must be integers".to_string())
                }
            }
            ExprKind::If { kind: if_kind, cond, then_expr, else_expr } => {
                if self.in_model {
                    let cond_ty = self.check_expr(cond)?;
                    let cond_stripped = self.strip_privacy(&cond_ty);
                    if let Type::Tensor { .. } = cond_stripped {
                        return Err("error[MD003]: runtime tensor condition must use explicit dynamic operator `tensor.cond`".to_string());
                    }
                }

                let cond_ty = self.check_expr(cond)?;
                if cond_ty != Type::Bool {
                    return Err("if condition must be bool".to_string());
                }
                let then_ty = self.check_expr(then_expr)?;

                match if_kind {
                    // ===== Normal：老规矩，else 必须有，两分支类型必须一致 =====
                    IfKind::Normal => {
                        let else_ty = if let Some(else_expr) = else_expr {
                            self.check_expr(else_expr)?
                        } else {
                            // 关键：不写 lack 又不写 else，依旧是错——这门语言
                            // 喜欢显式声明，"没有 else"这件事必须靠 `lack if`
                            // 明说，不能靠"忘了写"蒙混过去。
                            return Err(
                                "if expression requires an else branch（如果这个 if 不需要产出值、纯粹是副作用，请显式写成 `lack if`）"
                                    .to_string(),
                            );
                        };
                        if !self.types_equal_with_privacy(&then_ty, &else_ty) {
                            return Err(format!(
                                "if branches have different types: then = {:?}, else = {:?}",
                                then_ty, else_ty
                            ));
                        }
                        let joined_tag = self.join_privacy_labels(&then_ty, &else_ty);
                        let base_ty = self.strip_privacy(&then_ty);
                        Ok(self.apply_privacy_tag(base_ty, joined_tag))
                    }
                    // ===== Lack：反过来，else 必须没有，then 必须是 Unit =====
                    IfKind::Lack => {
                        if else_expr.is_some() {
                            return Err(
                                "`lack if` must not have an else branch（既然写了 else，就该用普通 if，不要用 lack if）"
                                    .to_string(),
                            );
                        }
                        let then_inner = self.strip_privacy(&then_ty);
                        if then_inner != Type::Unit {
                            return Err(format!(
                                "`lack if` 的 then 分支必须是 Unit 类型（纯副作用、不产出值），得到 {:?}（如果这个 if 需要产出值，请改成普通 if 并补上 else 分支）",
                                then_ty
                            ));
                        }
                        Ok(Type::Unit)
                    }
                }
            }
            ExprKind::ArrayLiteral(elements) => {
                if elements.is_empty() {
                    return Err("array literal cannot be empty".to_string());
                }
                let mut values = Vec::new();
                for elem in elements {
                    let ty = self.check_expr(elem)?;
                    if let Type::I32 | Type::I64 = ty {
                        if let Some(v) = self.eval_const_int_expr(elem) {
                            values.push(v);
                        } else {
                            return Err(format!(
                                "array element is not a constant integer: {:?}",
                                elem
                            ));
                        }
                    } else {
                        return Err(format!(
                            "array literal elements must be integers, got {:?}",
                            ty
                        ));
                    }
                }
                Ok(Type::ConstIntArray(values))
            }
            ExprKind::EnumVariantAccess { enum_name, variant_name } => {
                let enum_def = self
                    .enums
                    .get(enum_name)
                    .ok_or_else(|| format!("undefined enum: {}", enum_name))?;
                if !enum_def.variants.iter().any(|v| v.name == *variant_name) {
                    return Err(format!(
                        "enum {} has no variant named {}",
                        enum_name, variant_name
                    ));
                }
                Ok(Type::Enum(enum_name.clone()))
            }
            // ---- EnumVariantConstruction：委托给 check_enum_variant_construction，
            // expected 传 None，即"没有外部期望类型提示"这个默认情况。
            ExprKind::EnumVariantConstruction { enum_name, variant_name, args } => {
                self.check_enum_variant_construction(enum_name, variant_name, args, None)
            }
            // ---- Match：模式检查挪到了 pattern.rs 的 check_match_expr 里，
            // 这里只是委托调用。
            ExprKind::Match(match_expr) => self.check_match_expr(match_expr),
            // 关键修复：之前这里完全无视 unsafe 块里到底写了什么，无条件
            // 判成 I32。`as_str` 里 `unsafe { from_utf8_unchecked(...) }`
            // 这种写法，不管内部表达式真实类型是什么，永远被判成 I32，
            // 跟函数声明的 &str 返回类型对不上——这是本轮排查历史遗留
            // 问题时挖出的最严重的一处，不是"某个具体方法漏判"，是整个
            // unsafe-块-当表达式用 这条路径从一开始就没接对，改成真的去
            // 检查 block 内部内容。
            ExprKind::UnsafeBlock(unsafe_stmt) => self.check_block(&unsafe_stmt.body),
        };

        if let Ok(ty) = &result {
            self.expr_types.insert(expr.id, ty.clone());
        }
        result
    }
}
