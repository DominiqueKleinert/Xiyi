// elaborate.rs
use crate::hir::*;
use crate::ast::{BinaryOp, Literal, Pattern, Type, UnaryOp, IfKind, UnsafeKind};

/// 从容器类型推导"元素类型"
/// 支持 Vec<T>、&[T]、Option<T> 等单参数泛型，及切片和引用
fn extract_elem_type(ty: &Type) -> Type {
    match ty {
        Type::Generic(name, args) if args.len() == 1 => args[0].clone(),
        Type::Ref { inner, .. } => extract_elem_type(inner),
        Type::Slice(inner) => (**inner).clone(),
        // 关键修复：这条路径意味着 for 循环的迭代对象根本不是已知的
        // 容器形状（sema 应该已经拦住了这种情况，走到这里说明两边检查
        // 不一致）。之前没有真正的 Type::Never 时，只能退回 Unit 权充
        // ——但 Unit 是一个真实、具体的类型，用它意味着"这里的元素就是
        // ()"，会把一个本该报错的情况悄悄伪装成一个合法结果，继续往下
        // 游传播还可能诱发出跟 Unit 相关但毫不相干的二次报错。Never 才
        // 是诚实的表达："这里推不出真正的元素类型，也没有能通过合法路径
        // 产生的值"——它能跟任何类型统一，不会在这里制造新的假类型错误，
        // 又不会假装自己是一个"正常"的具体类型。
        _ => Type::Never,
    }
}

// ===== 常用 Pattern 构造 helper =====
//
// expand_for / expand_try 里反复出现 `Pattern::EnumVariantWithBinding {
// enum_name: "Option".to_string(), variant_name: "Some".to_string(), ... }`
// 这种样板，抽成四个小函数，调用点直接读出"这是 Some/None/Ok/Err 模式"，
// 不用每次都在一堆字符串字面量里确认 enum_name/variant_name 有没有抄对。

fn some_pattern(binding: &str) -> Pattern {
    Pattern::EnumVariantWithBinding {
        enum_name: "Option".to_string(),
        variant_name: "Some".to_string(),
        binding: binding.to_string(),
    }
}

fn none_pattern() -> Pattern {
    Pattern::EnumVariant {
        enum_name: "Option".to_string(),
        variant_name: "None".to_string(),
    }
}

fn ok_pattern(binding: &str) -> Pattern {
    Pattern::EnumVariantWithBinding {
        enum_name: "Result".to_string(),
        variant_name: "Ok".to_string(),
        binding: binding.to_string(),
    }
}

fn err_pattern(binding: &str) -> Pattern {
    Pattern::EnumVariantWithBinding {
        enum_name: "Result".to_string(),
        variant_name: "Err".to_string(),
        binding: binding.to_string(),
    }
}

// ===== 展开器上下文 =====

struct ElaborateContext {
    return_ty: Option<Type>,
    temp_counter: usize,
}

impl ElaborateContext {
    fn new(return_ty: Option<Type>) -> Self {
        Self {
            return_ty,
            temp_counter: 0,
        }
    }

    /// 生成一个唯一的临时变量名
    fn next_temp(&mut self) -> String {
        let id = self.temp_counter;
        self.temp_counter += 1;
        format!("__elab_{}", id)
    }

    /// 判断当前函数返回类型是否支持 `?`
    fn supports_try(&self) -> bool {
        match &self.return_ty {
            Some(Type::Generic(name, _)) => name == "Result" || name == "Option",
            _ => false,
        }
    }

    /// 判断返回类型是否是 `Result`
    fn is_result(&self) -> bool {
        match &self.return_ty {
            Some(Type::Generic(name, _)) => name == "Result",
            _ => false,
        }
    }

    /// 判断返回类型是否是 `Option`
    fn is_option(&self) -> bool {
        match &self.return_ty {
            Some(Type::Generic(name, _)) => name == "Option",
            _ => false,
        }
    }

    /// 获取错误类型参数（用于 `?` 的 `Err(e) => return Err(e.into())`）
    /// 如果返回类型是 `Result<T, E>`，返回 `E`；否则返回 `Type::Never`
    /// ——这个分支只在 is_result() 已经确认 return_ty 是
    /// `Some(Generic("Result", _))` 之后、args 长度却不是 2 时才会走到，
    /// 属于"类型系统已经检查过、理论上不该发生"的防御性兜底，用 Never
    /// 如实标记，不用 Unit 假装这是一个正常情况。
    fn error_type(&self) -> Type {
        match &self.return_ty {
            Some(Type::Generic(name, args)) if name == "Result" && args.len() == 2 => {
                args[1].clone()
            }
            _ => Type::Never,
        }
    }

    // ===== 构造 HirExpr 的 helper =====
    //
    // elaborate 阶段生成的 HirExpr 绝大多数是编译器合成的节点：没有真实
    // 的隐私标签（privacy_tag: None）、sensitivity 恒为 Unknown。以前每
    // 次都要把这 6 个字段抄一遍，现在收口到这几个函数里。

    fn mk_expr(&self, kind: HirExprKind, ty: Type, effects: EffectSet, span: Span) -> HirExpr {
        HirExpr {
            kind,
            ty,
            privacy_tag: None,
            sensitivity: Sensitivity::Unknown,
            effects,
            span,
        }
    }

    fn mk_expr_default(&self, kind: HirExprKind, ty: Type, span: Span) -> HirExpr {
        self.mk_expr(kind, ty, EffectSet::default(), span)
    }

    fn mk_call(&self, func: &str, args: Vec<HirCallArg>, ty: Type, span: Span) -> HirExpr {
        self.mk_expr_default(
            HirExprKind::Call {
                qualifier: None,
                func: func.to_string(),
                generic_args: Vec::new(),
                args,
                is_method: true,
            },
            ty,
            span,
        )
    }

    fn mk_ident(&self, name: &str, ty: Type, span: Span) -> HirExpr {
        self.mk_expr_default(HirExprKind::Ident(name.to_string()), ty, span)
    }

    /// `Ok(temp) => temp` / `Some(temp) => temp` 这类分支：把被匹配值原样
    /// 绑定到新名字上。跟 mk_ident 的区别是这里必须原样继承被匹配表达式
    /// 的类型、隐私标签、effects——它绑定的是一个真实的值，不是凭空合成
    /// 出来的临时量，直接套 mk_ident（privacy_tag 恒 None、effects 恒
    /// default）会把这些信息悄悄丢掉。
    fn mk_bound_ident(&self, name: &str, from: &HirExpr) -> HirExpr {
        HirExpr {
            kind: HirExprKind::Ident(name.to_string()),
            ty: from.ty.clone(),
            privacy_tag: from.privacy_tag.clone(),
            sensitivity: Sensitivity::Unknown,
            effects: from.effects.clone(),
            span: from.span,
        }
    }

    /// expand_for 的 `None => break`、expand_try 的 `Err(e) => return
    /// Err(e.into())` / `None => return None` 都是同一个模式："用一个
    /// Block 包一条发散语句（break/return），把这条分支的类型标成
    /// Never"。三处除了 Block 里装的语句不同，其余完全一样，抽成一个
    /// helper。
    fn mk_diverging_block(&self, stmts: Vec<HirStmt>, span: Span) -> HirExpr {
        self.mk_expr_default(HirExprKind::Block(HirBlock { stmts, span }), Type::Never, span)
    }
}

// ===== 展开器主结构体 =====

pub struct Elaborate;

impl Elaborate {
    pub fn elaborate(program: HirProgram) -> Result<HirProgram, String> {
        let mut ctx = ElaborateContext::new(None);
        Self::elaborate_program(program, &mut ctx)
    }

    // ===== 顶层遍历 =====

    fn elaborate_program(mut program: HirProgram, ctx: &mut ElaborateContext) -> Result<HirProgram, String> {
        // 展开顶层函数
        let mut fns = Vec::new();
        for f in program.fns {
            fns.push(Self::elaborate_fn(f, ctx)?);
        }
        program.fns = fns;

        // 展开 model 中的函数
        let mut models = Vec::new();
        for m in program.models {
            let mut functions = Vec::new();
            for f in m.functions {
                functions.push(Self::elaborate_fn(f, ctx)?);
            }
            models.push(HirModel {
                functions,
                ..m
            });
        }
        program.models = models;

        // 展开 implement 块中的函数
        let mut impls = Vec::new();
        for imp in program.impls {
            let mut functions = Vec::new();
            for f in imp.functions {
                functions.push(Self::elaborate_fn(f, ctx)?);
            }
            impls.push(HirImplement {
                functions,
                ..imp
            });
        }
        program.impls = impls;

        // interface 方法无 body，consts 的值暂不展开
        Ok(program)
    }

    // ===== 函数级展开 =====

    fn elaborate_fn(mut f: HirFn, ctx: &mut ElaborateContext) -> Result<HirFn, String> {
        let mut fn_ctx = ElaborateContext::new(f.return_type.clone());
        let body = Self::elaborate_block(f.body, &mut fn_ctx)?;
        f.body = body;
        Ok(f)
    }

    // ===== 块级展开 =====

    fn elaborate_block(mut block: HirBlock, ctx: &mut ElaborateContext) -> Result<HirBlock, String> {
        let mut new_stmts = Vec::new();
        for stmt in block.stmts {
            // 关键修复：原来这里有一段 `if let HirStmt::Block { stmts, .. } =
            // expanded { ... }` 的"拍平"逻辑，但 hir.rs 里 HirStmt 根本没有
            // Block 这个变体（只有 Let/Expr/Return/While/For/Assign/Loop/
            // Break/UnsafeBlock 九种），编译不过。而且 elaborate_stmt 的每
            // 个分支都老老实实返回单个 HirStmt，从不会产出需要"拍平"的
            // 东西，这段本来就是针对一个不存在场景写的死代码，直接去掉。
            let expanded = Self::elaborate_stmt(stmt, ctx)?;
            new_stmts.push(expanded);
        }
        block.stmts = new_stmts;
        Ok(block)
    }

    // ===== 语句级展开 =====

    fn elaborate_stmt(stmt: HirStmt, ctx: &mut ElaborateContext) -> Result<HirStmt, String> {
        match stmt {
            HirStmt::For { var, iterable, body, span } => {
                Self::expand_for(var, iterable, body, span, ctx)
            }

            HirStmt::Let { name, ty, init, mutable, persist, span } => {
                let init = Self::elaborate_expr(init, ctx)?;
                Ok(HirStmt::Let { name, ty, init, mutable, persist, span })
            }
            HirStmt::Expr { expr, span } => {
                let expr = Self::elaborate_expr(expr, ctx)?;
                Ok(HirStmt::Expr { expr, span })
            }
            HirStmt::Return { expr, span } => {
                let expr = expr.map(|e| Self::elaborate_expr(e, ctx)).transpose()?;
                Ok(HirStmt::Return { expr, span })
            }
            HirStmt::While { cond, body, span } => {
                let cond = Self::elaborate_expr(cond, ctx)?;
                let body = Self::elaborate_block(body, ctx)?;
                Ok(HirStmt::While { cond, body, span })
            }
            HirStmt::Assign { target, expr, span } => {
                let target = Self::elaborate_expr(*target, ctx)?;
                let expr = Self::elaborate_expr(expr, ctx)?;
                Ok(HirStmt::Assign { target: Box::new(target), expr, span })
            }
            HirStmt::Loop { body, span } => {
                let body = Self::elaborate_block(body, ctx)?;
                Ok(HirStmt::Loop { body, span })
            }
            HirStmt::Break { span } => Ok(HirStmt::Break { span }),
            HirStmt::UnsafeBlock { kind, body, span } => {
                let body = Self::elaborate_block(body, ctx)?;
                Ok(HirStmt::UnsafeBlock { kind, body, span })
            }
        }
    }

    // ===== for 循环展开 =====
    //
    // 原来 expand_for 接近 180 行，一个函数里同时干了三件事：造
    // `let __iter = iterable.into_iter()`、造 `match __iter.next() { ... }`、
    // 把两者拼成最终的 block。拆成 build_into_iter_let / build_for_next_match
    // 两个子步骤后，expand_for 本身只剩"组装"逻辑，每一步在干什么一眼
    // 看得出来。

    fn expand_for(
        var: String,
        iterable: HirExpr,
        body: HirBlock,
        span: Span,
        ctx: &mut ElaborateContext,
    ) -> Result<HirStmt, String> {
        let iter_var = ctx.next_temp();
        let elem_ty = extract_elem_type(&iterable.ty);
        let iterable_ty = iterable.ty.clone();
        let iterable_effects = iterable.effects.clone();

        // 1-2. let __iter = iterable.into_iter();
        let let_stmt = Self::build_into_iter_let(iter_var.clone(), iterable, span.clone(), ctx);

        // 3-6. match __iter.next() { Some(var) => { body }, None => { break } }
        let (match_expr, match_effects) = Self::build_for_next_match(
            &iter_var,
            elem_ty,
            var,
            body,
            iterable_ty,
            iterable_effects,
            span.clone(),
            ctx,
        );

        // 7. loop { match ... }
        let loop_block = HirBlock {
            stmts: vec![HirStmt::Expr { expr: match_expr, span: span.clone() }],
            span: span.clone(),
        };
        let loop_stmt = HirStmt::Loop {
            body: loop_block,
            span: span.clone(),
        };

        // 8. 追加 Unit 表达式，强制块类型为 ()
        let unit_expr = ctx.mk_expr_default(HirExprKind::Literal(Literal::Unit), Type::Unit, span.clone());
        let block = HirBlock {
            stmts: vec![let_stmt, loop_stmt, HirStmt::Expr { expr: unit_expr, span: span.clone() }],
            span: span.clone(),
        };

        // 外层块的整体副作用 = let_stmt(无) + match_effects + unit(无)
        Ok(HirStmt::Expr {
            expr: ctx.mk_expr(HirExprKind::Block(block), Type::Unit, match_effects, span.clone()),
            span,
        })
    }

    fn build_into_iter_let(
        iter_var: String,
        iterable: HirExpr,
        span: Span,
        ctx: &ElaborateContext,
    ) -> HirStmt {
        let ty = iterable.ty.clone();
        let effects = iterable.effects.clone();
        let into_iter_call = ctx.mk_expr(
            HirExprKind::Call {
                qualifier: None,
                func: "into_iter".to_string(),
                generic_args: Vec::new(),
                args: vec![HirCallArg::Positional(iterable)],
                is_method: true,
            },
            ty,
            effects,
            span.clone(),
        );
        HirStmt::Let {
            name: iter_var,
            ty: None,
            init: into_iter_call,
            mutable: true,
            persist: false,
            span,
        }
    }

    fn build_for_next_match(
        iter_var: &str,
        elem_ty: Type,
        var: String,
        body: HirBlock,
        iterable_ty: Type,
        iterable_effects: EffectSet,
        span: Span,
        ctx: &ElaborateContext,
    ) -> (HirExpr, EffectSet) {
        // 关键修复：elem_ty 后面 match_expr 的类型还要再用一次，Type
        // 没有 derive Copy，这里不 clone 的话第二次用就是 "use of moved
        // value"（E0382），编译不过。
        let next_call = ctx.mk_expr(
            HirExprKind::Call {
                qualifier: None,
                func: "next".to_string(),
                generic_args: Vec::new(),
                args: vec![HirCallArg::Positional(ctx.mk_ident(iter_var, iterable_ty, span.clone()))],
                is_method: true,
            },
            Type::Generic("Option".to_string(), vec![elem_ty.clone()]),
            // 继续传播迭代对象自身的副作用
            iterable_effects,
            span.clone(),
        );
        let next_effects = next_call.effects.clone();

        let body_effects = Self::collect_effects_from_block(&body);
        let some_arm = HirMatchArm {
            pattern: some_pattern(&var),
            expr: ctx.mk_expr(HirExprKind::Block(body), Type::Unit, body_effects.clone(), span.clone()),
        };

        // 关键修复：break_expr 的类型之前标的是 Type::Unit——但这里执行
        // 的是 `{ break; }`，跟 ast.rs 里 Type::Never 那条注释举的例子
        // 一模一样（"`return`/`break` 表达式本身...都可以标成 Never"）。
        // break 会直接跳出这个 loop，根本不会把控制流交回这个 match
        // 分支、更不会产出一个 Unit 值——标成 Unit 只是因为当初还没有
        // Never，且这里恰好跟 some_arm（真的是 Unit）拼出来的两个分支
        // "看起来"类型一致，能蒙混过关。现在有了 Never，就没必要再用
        // Unit 顶替了。
        let break_expr = ctx.mk_diverging_block(vec![HirStmt::Break { span: span.clone() }], span.clone());
        let none_arm = HirMatchArm {
            pattern: none_pattern(),
            expr: break_expr.clone(),
        };

        // 关键修复：match_expr 马上要在下面被整个移进 loop_block 里
        // （HirStmt::Expr { expr: match_expr, .. }），移动之后就不能再
        // 通过 match_expr.effects 读它了——HirExpr 没有实现 Copy，
        // "值被移动"和"字段还能借用"是两码事，rustc 会直接报
        // "borrow of moved value"。这里先把即将写进 match_expr.effects
        // 的值 clone 一份留给调用方（loop_effects），不用等移动之后再
        // 回头去"读"一个已经不完整的值。
        let match_effects = EffectSet::merge(&[&next_effects, &body_effects, &break_expr.effects]);
        let match_expr = ctx.mk_expr(
            HirExprKind::Match {
                cond: Box::new(next_call),
                arms: vec![some_arm, none_arm],
            },
            Type::Generic("Option".to_string(), vec![elem_ty]),
            match_effects.clone(),
            span,
        );
        (match_expr, match_effects)
    }

    // ===== 表达式级展开 =====

    fn elaborate_expr(mut expr: HirExpr, ctx: &mut ElaborateContext) -> Result<HirExpr, String> {
        match expr.kind {
            // ---- ? 操作符展开 ----
            HirExprKind::Call { qualifier, func, generic_args, args, is_method } => {
                if qualifier.is_none() && !is_method && func == "try" && args.len() == 1 {
                    let arg = match &args[0] {
                        HirCallArg::Positional(e) => e.clone(),
                        HirCallArg::Named(_, e) => e.clone(),
                    };
                    return Self::expand_try(arg, ctx);
                }

                let new_args = Self::elaborate_call_args(args, ctx)?;
                expr.kind = HirExprKind::Call {
                    qualifier,
                    func,
                    generic_args,
                    args: new_args,
                    is_method,
                };
                Ok(expr)
            }

            // ---- 其他表达式：递归展开子表达式 ----
            HirExprKind::Literal(_) => Ok(expr),
            HirExprKind::Ident(_) => Ok(expr),
            HirExprKind::Sym(_) => Ok(expr),

            HirExprKind::BinaryOp { op, left, right } => {
                let left = Self::elaborate_boxed(left, ctx)?;
                let right = Self::elaborate_boxed(right, ctx)?;
                expr.kind = HirExprKind::BinaryOp { op, left, right };
                Ok(expr)
            }

            HirExprKind::Unary { op, expr: inner } => {
                expr.kind = HirExprKind::Unary { op, expr: Self::elaborate_boxed(inner, ctx)? };
                Ok(expr)
            }

            HirExprKind::Cast { expr: inner, ty } => {
                expr.kind = HirExprKind::Cast { expr: Self::elaborate_boxed(inner, ctx)?, ty };
                Ok(expr)
            }

            HirExprKind::Block(block) => {
                let block = Self::elaborate_block(block, ctx)?;
                expr.kind = HirExprKind::Block(block);
                Ok(expr)
            }

            HirExprKind::Match { cond, arms } => {
                let cond = Self::elaborate_boxed(cond, ctx)?;
                let mut new_arms = Vec::new();
                for arm in arms {
                    let arm_expr = Self::elaborate_expr(arm.expr, ctx)?;
                    new_arms.push(HirMatchArm {
                        pattern: arm.pattern,
                        expr: arm_expr,
                    });
                }
                expr.kind = HirExprKind::Match { cond, arms: new_arms };
                Ok(expr)
            }

            HirExprKind::If { kind, cond, then_expr, else_expr } => {
                let cond = Self::elaborate_boxed(cond, ctx)?;
                let then_expr = Self::elaborate_boxed(then_expr, ctx)?;
                let else_expr = else_expr.map(|e| Self::elaborate_boxed(e, ctx)).transpose()?;
                expr.kind = HirExprKind::If { kind, cond, then_expr, else_expr };
                Ok(expr)
            }

            HirExprKind::StructInit { struct_name, generic_args, fields } => {
                let mut new_fields = Vec::new();
                for (name, e) in fields {
                    new_fields.push((name, Self::elaborate_expr(e, ctx)?));
                }
                expr.kind = HirExprKind::StructInit {
                    struct_name,
                    generic_args,
                    fields: new_fields,
                };
                Ok(expr)
            }

            HirExprKind::FieldAccess { struct_expr, field_name } => {
                expr.kind = HirExprKind::FieldAccess {
                    struct_expr: Self::elaborate_boxed(struct_expr, ctx)?,
                    field_name,
                };
                Ok(expr)
            }

            HirExprKind::Index { expr: base, index } => {
                let base = Self::elaborate_boxed(base, ctx)?;
                let index = Self::elaborate_boxed(index, ctx)?;
                expr.kind = HirExprKind::Index { expr: base, index };
                Ok(expr)
            }

            HirExprKind::Range { start, end } => {
                let start = Self::elaborate_boxed(start, ctx)?;
                let end = Self::elaborate_boxed(end, ctx)?;
                expr.kind = HirExprKind::Range { start, end };
                Ok(expr)
            }

            HirExprKind::Closure { param, body } => {
                expr.kind = HirExprKind::Closure { param, body: Self::elaborate_boxed(body, ctx)? };
                Ok(expr)
            }

            HirExprKind::ArrayLiteral(elements) => {
                let mut new_elements = Vec::new();
                for e in elements {
                    new_elements.push(Self::elaborate_expr(e, ctx)?);
                }
                expr.kind = HirExprKind::ArrayLiteral(new_elements);
                Ok(expr)
            }

            HirExprKind::UnsafeBlock { kind, body, span } => {
                let body = Self::elaborate_block(body, ctx)?;
                expr.kind = HirExprKind::UnsafeBlock { kind, body, span };
                Ok(expr)
            }

            HirExprKind::EnumVariantAccess { .. } => Ok(expr),
            HirExprKind::EnumVariantConstruction { enum_name, generic_args, variant_name, args } => {
                let new_args = Self::elaborate_call_args(args, ctx)?;
                expr.kind = HirExprKind::EnumVariantConstruction {
                    enum_name,
                    generic_args,
                    variant_name,
                    args: new_args,
                };
                Ok(expr)
            }

            HirExprKind::LackSlice(_) => Ok(expr),
        }
    }

    /// 递归展开一个 `Box<HirExpr>`。Unary/Cast/FieldAccess/Closure/Index/
    /// Range/BinaryOp/If/Match 里全是"拆箱 -> elaborate_expr -> 再装箱"
    /// 这一个动作，抽出来后每个 match arm 只需要关心自己特有的字段，不用
    /// 每次都重复 `Box::new(Self::elaborate_expr(*x, ctx)?)`。
    ///
    /// 注：这里没有像建议里那样引入一个带默认递归 + 通配 `_ => expr.kind`
    /// 的 HirExprFolder trait。原因是 elaborate_expr 目前对 HirExprKind
    /// 的所有变体都显式列出、没有通配分支——这不是疏忽，是故意的：hir.rs
    /// 每加一个新变体，这里的 match 就会因为"非穷尽"编译不过，逼着我们
    /// 回来决定新变体要不要特殊处理。这正是之前在 mir.rs 里发现
    /// `MirRvalue::Discriminant` 需要单独处理的方式——靠的就是穷尽匹配。
    /// 通配分支会悄悄放弃这层保护：新变体默认"什么都不做地透传"，如果
    /// 它其实需要 elaborate（比如内部也带 Box<HirExpr>），编译器不会提醒，
    /// 只会在运行时表现出诡异行为。所以这里选择只抽取"重复的递归动作"，
    /// 保留穷尽匹配本身。
    fn elaborate_boxed(e: Box<HirExpr>, ctx: &mut ElaborateContext) -> Result<Box<HirExpr>, String> {
        Ok(Box::new(Self::elaborate_expr(*e, ctx)?))
    }

    /// Call / EnumVariantConstruction 展开各自参数列表的逻辑完全一样。
    fn elaborate_call_args(
        args: Vec<HirCallArg>,
        ctx: &mut ElaborateContext,
    ) -> Result<Vec<HirCallArg>, String> {
        let mut new_args = Vec::new();
        for arg in args {
            new_args.push(match arg {
                HirCallArg::Positional(e) => HirCallArg::Positional(Self::elaborate_expr(e, ctx)?),
                HirCallArg::Named(name, e) => HirCallArg::Named(name, Self::elaborate_expr(e, ctx)?),
            });
        }
        Ok(new_args)
    }

    // ===== ? 操作符展开 =====
    //
    // 原来 expand_try 接近 200 行，Result 和 Option 两条分支各自手写一遍
    // "构造 Ok/Some 分支 -> 构造 Err/None 提前 return 分支 -> 拼 match"，
    // 拆成 build_result_try / build_option_try 后，expand_try 本身只剩
    // "选哪条路"。

    fn expand_try(expr: HirExpr, ctx: &mut ElaborateContext) -> Result<HirExpr, String> {
        if !ctx.supports_try() {
            return Err(format!(
                "`?` cannot be used in a function that returns {:?}; \
                 only functions returning `Result<T, E>` or `Option<T>` support `?`",
                ctx.return_ty
            ));
        }

        let temp_var = ctx.next_temp();

        if ctx.is_result() {
            Self::build_result_try(expr, temp_var, ctx)
        } else if ctx.is_option() {
            Self::build_option_try(expr, temp_var, ctx)
        } else {
            Err("`?` requires Result or Option return type".to_string())
        }
    }

    fn build_result_try(
        expr: HirExpr,
        temp_var: String,
        ctx: &mut ElaborateContext,
    ) -> Result<HirExpr, String> {
        let span = expr.span;

        // Ok(temp) => temp
        let ok_arm = HirMatchArm {
            pattern: ok_pattern(&temp_var),
            expr: ctx.mk_bound_ident(&temp_var, &expr),
        };

        // Err(e) => return Err(e.into())
        let err_var = ctx.next_temp();
        let into_call = ctx.mk_call(
            "into",
            vec![HirCallArg::Positional(ctx.mk_ident(&err_var, ctx.error_type(), span))],
            ctx.error_type(),
            span,
        );

        // 关键修复：这里原来直接把 `Result::Err(e.into())` 这个值当成
        // err_arm 的结果，还把它的类型标成整个函数的返回类型
        // Result<T, E>——但 ok_arm 的类型是 T，两个分支类型对不上
        // （一个是 T，一个是 Result<T, E>），这本该是个错误。真正的 `?`
        // 语义是"Err 分支要提前 return"（对应 Rust 里
        // `Err(e) => return Err(e.into())`），return 会让这条分支发散、
        // 根本不产出 T 类型的值——之前没有真正的 Type::Never，没法诚实
        // 地把这条分支标出来，只能靠"各自设自己的类型、指望没人检查"
        // 蒙混过关。现在用 mk_diverging_block 包一条 Return 语句，把
        // 分支类型标成 Never，就能跟 ok_arm 的 T 正常统一了。
        let err_return_value = ctx.mk_expr_default(
            HirExprKind::Call {
                qualifier: Some("Result".to_string()),
                func: "Err".to_string(),
                generic_args: Vec::new(),
                args: vec![HirCallArg::Positional(into_call)],
                is_method: false,
            },
            // 这里才是"要 return 出去的那个值"自身的类型，即
            // Result<T, E>；unwrap_or 的 Never 兜底只在 is_result()
            // 已经确认过 return_ty 形状之后，理论上不该走到。
            ctx.return_ty.clone().unwrap_or(Type::Never),
            span,
        );
        let err_arm = HirMatchArm {
            pattern: err_pattern(&err_var),
            expr: ctx.mk_diverging_block(vec![HirStmt::Return { expr: Some(err_return_value), span }], span),
        };

        let match_effects = EffectSet::merge(&[&expr.effects, &ok_arm.expr.effects, &err_arm.expr.effects]);
        let result_ty = match &ctx.return_ty {
            Some(Type::Generic(name, args)) if name == "Result" && args.len() == 2 => args[0].clone(),
            // is_result() 已经确认过形状，这条分支理论上不该走到，用
            // Never 如实标记"不该发生"。
            _ => Type::Never,
        };

        Ok(ctx.mk_expr(
            HirExprKind::Match { cond: Box::new(expr), arms: vec![ok_arm, err_arm] },
            result_ty,
            match_effects,
            span,
        ))
    }

    fn build_option_try(
        expr: HirExpr,
        temp_var: String,
        ctx: &mut ElaborateContext,
    ) -> Result<HirExpr, String> {
        let span = expr.span;

        // Some(temp) => temp
        let some_arm = HirMatchArm {
            pattern: some_pattern(&temp_var),
            expr: ctx.mk_bound_ident(&temp_var, &expr),
        };

        // 关键修复：跟上面 Result 分支同一个问题、同一个手法——
        // None 分支也必须是"提前 return None"，而不是把
        // `Option::None` 这个值直接当 match 分支结果（它的类型是
        // Option<T>，跟 some_arm 的 T 对不上）。
        let none_return_value = ctx.mk_expr_default(
            HirExprKind::Call {
                qualifier: Some("Option".to_string()),
                func: "None".to_string(),
                generic_args: Vec::new(),
                args: Vec::new(),
                is_method: false,
            },
            ctx.return_ty.clone().unwrap_or(Type::Never),
            span,
        );
        let none_arm = HirMatchArm {
            pattern: none_pattern(),
            expr: ctx.mk_diverging_block(vec![HirStmt::Return { expr: Some(none_return_value), span }], span),
        };

        let match_effects = EffectSet::merge(&[&expr.effects, &some_arm.expr.effects, &none_arm.expr.effects]);
        let result_ty = match &ctx.return_ty {
            Some(Type::Generic(name, args)) if name == "Option" && args.len() == 1 => args[0].clone(),
            // is_option() 已经确认过形状，这条分支理论上不该走到，用
            // Never 如实标记"不该发生"，不用 Unit 假装这是个正常结果。
            _ => Type::Never,
        };

        Ok(ctx.mk_expr(
            HirExprKind::Match { cond: Box::new(expr), arms: vec![some_arm, none_arm] },
            result_ty,
            match_effects,
            span,
        ))
    }

    // ===== 辅助：收集 HirBlock 中所有语句的 EffectSet =====
    //
    // 原来这里是接近 60 行的手抄 OR：每种 HirStmt 变体都单独把 has_io /
    // has_rng / has_ai / has_ffi / has_panic 五个字段各 or 一遍。而
    // hir.rs 已经有语义完全一样的 EffectSet::merge。现在改成"先摘出这个
    // block（递归地）涉及到的所有 HirExpr 引用，再一次性交给
    // EffectSet::merge"，合并逻辑只在一个地方维护。

    fn collect_effects_from_block(block: &HirBlock) -> EffectSet {
        let exprs = Self::block_exprs(block);
        let refs: Vec<&EffectSet> = exprs.iter().map(|e| &e.effects).collect();
        EffectSet::merge(&refs)
    }

    fn block_exprs(block: &HirBlock) -> Vec<&HirExpr> {
        let mut all = Vec::new();
        for stmt in &block.stmts {
            all.extend(Self::stmt_exprs(stmt));
        }
        all
    }

    fn stmt_exprs(stmt: &HirStmt) -> Vec<&HirExpr> {
        match stmt {
            HirStmt::Expr { expr, .. } => vec![expr],
            HirStmt::Let { init, .. } => vec![init],
            HirStmt::Return { expr: Some(e), .. } => vec![e],
            HirStmt::Return { expr: None, .. } => vec![],
            HirStmt::While { cond, body, .. } => {
                let mut v = vec![cond];
                v.extend(Self::block_exprs(body));
                v
            }
            HirStmt::For { body, .. } => Self::block_exprs(body),
            HirStmt::Loop { body, .. } => Self::block_exprs(body),
            HirStmt::Assign { target, expr, .. } => vec![target.as_ref(), expr],
            HirStmt::UnsafeBlock { body, .. } => Self::block_exprs(body),
            HirStmt::Break { .. } => vec![],
        }
    }
}
