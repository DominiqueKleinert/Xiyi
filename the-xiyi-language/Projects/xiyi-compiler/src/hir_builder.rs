use crate::ast::*;
use crate::hir::*;
use std::collections::{BTreeSet, HashMap};

type BuildResult<T> = Result<T, String>;

// privacy_tag() 现在定义在 ast.rs 的 impl Type 里（上一轮先临时放在这个
// 文件是因为那次只同步到 hir_builder.rs，拿到 ast.rs 之后就该搬回它本该
// 在的地方）。这里靠 `use crate::ast::*` 直接拿到这个方法，不用再自己声明。

pub struct HirBuilder;

impl HirBuilder {
    fn ok<T>(val: T) -> BuildResult<T> {
        Ok(val)
    }

    // 抽出来复用，避免 build_struct/build_enum/build_fn/build_implement/build_interface
    // 各写一份、以后 GenericParam 加新变体时到处漏改
    // 注：现在把 bounds 也透传进 HIR 了（ast::GenericParam::Type 已经带 bounds），
    // 之前那版直接 `..` 丢弃 bounds 只是为了先能编译过，这版补上。
    fn build_generic_params(generic_params: &[GenericParam]) -> Vec<HirGenericParam> {
        generic_params
            .iter()
            .map(|gp| match gp {
                GenericParam::Type { name, bounds } => HirGenericParam {
                    name: name.clone(),
                    bounds: bounds.clone(),
                },
            })
            .collect()
    }

    // 通用 effect 合并 helper：接收任意一批 &HirExpr（数组字面量、Vec 迭代器、
    // chain 起来的可选项……都行），取出各自的 effects 交给 EffectSet::merge。
    // 之前 Call / EnumVariantConstruction / StructInit / ArrayLiteral / Match /
    // If 等分支各自手写一份 `let mut child_effects: Vec<&EffectSet> = ...; for
    // ... { child_effects.push(...) }`，现在统一走这一个函数。
    fn merge_effects<'a, I>(exprs: I) -> EffectSet
    where
        I: IntoIterator<Item = &'a HirExpr>,
    {
        let refs: Vec<&EffectSet> = exprs.into_iter().map(|e| &e.effects).collect();
        EffectSet::merge(&refs)
    }

    // Call / EnumVariantConstruction 的实参列表结构一样（都是 Vec<HirCallArg>），
    // 从中取出各参数表达式的写法也完全一样，抽出来避免两处重复。
    fn call_arg_expr(arg: &HirCallArg) -> &HirExpr {
        match arg {
            HirCallArg::Positional(e) => e,
            HirCallArg::Named(_, e) => e,
        }
    }

    fn merge_effects_from_call_args(args: &[HirCallArg]) -> EffectSet {
        Self::merge_effects(args.iter().map(Self::call_arg_expr))
    }

    // Call / EnumVariantConstruction 中把 ast::CallArg 逐个 build 成
    // HirCallArg 的逻辑也完全一样，一并抽出来。
    fn build_call_args(
        args: &[CallArg],
        expr_types: &HashMap<usize, Type>,
    ) -> BuildResult<Vec<HirCallArg>> {
        args.iter()
            .map(|arg| match arg {
                CallArg::Positional(e) => {
                    Self::ok(HirCallArg::Positional(Self::build_expr(e, expr_types)?))
                }
                CallArg::Named(name, e) => Self::ok(HirCallArg::Named(
                    name.clone(),
                    Self::build_expr(e, expr_types)?,
                )),
            })
            .collect()
    }

    fn merge_effects_from_block(block: &HirBlock) -> EffectSet {
        let mut merged = EffectSet::default();
        for stmt in &block.stmts {
            match stmt {
                HirStmt::Expr { expr, .. } => merged.merge_with(&expr.effects),
                HirStmt::Let { init, .. } => merged.merge_with(&init.effects),
                HirStmt::Return { expr: Some(e), .. } => merged.merge_with(&e.effects),
                HirStmt::Assign { target, expr, .. } => {
                    merged.merge_with(&target.effects);
                    merged.merge_with(&expr.effects);
                }
                HirStmt::While { cond, body, .. } => {
                    merged.merge_with(&cond.effects);
                    merged.merge_with(&Self::merge_effects_from_block(body));
                }
                HirStmt::For { iterable, body, .. } => {
                    merged.merge_with(&iterable.effects);
                    merged.merge_with(&Self::merge_effects_from_block(body));
                }
                HirStmt::Loop { body, .. } => {
                    merged.merge_with(&Self::merge_effects_from_block(body));
                }
                HirStmt::UnsafeBlock { body, .. } => {
                    merged.merge_with(&Self::merge_effects_from_block(body));
                }
                // Break 无子表达式，忽略
                _ => {}
            }
        }
        merged
    }

    pub fn build(
        program: &Program,
        expr_types: &HashMap<usize, Type>,
    ) -> BuildResult<HirProgram> {
        let mut models = Vec::new();
        let mut fns = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut consts = Vec::new();
        let mut protos = Vec::new();
        let mut impls = Vec::new();
        let mut interfaces = Vec::new();

        for item in &program.items {
            match item {
                Item::ModelDef(m) => models.push(Self::build_model(m, expr_types)?),
                Item::FnDef(f) => fns.push(Self::build_fn(f, expr_types, false)?),
                Item::StructDef(s) => structs.push(Self::build_struct(s)?),
                Item::EnumDef(e) => enums.push(Self::build_enum(e)?),
                Item::ConstDef(c) => consts.push(Self::build_const(c, expr_types)?),
                Item::ProtoDef(p) => protos.push(Self::build_proto(p)?),
                Item::Use(_) => {
                    // 暂不处理，后续实现模块解析时再使用
                }
                Item::Implement(imp) => {
                    impls.push(Self::build_implement(imp, expr_types)?);
                }
                Item::Interface(iface) => {
                    interfaces.push(Self::build_interface(iface)?);
                }
            }
        }

        Ok(HirProgram {
            models,
            fns,
            structs,
            enums,
            consts,
            protos,
            impls,
            interfaces,
        })
    }

    fn build_implement(
        imp: &ImplementDef,
        expr_types: &HashMap<usize, Type>,
    ) -> BuildResult<HirImplement> {
        let functions = imp
            .functions
            .iter()
            .map(|f| Self::build_fn(f, expr_types, false))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(HirImplement {
            generic_params: Self::build_generic_params(&imp.generic_params),
            target_type: imp.target_type.clone(),
            interface_name: imp.interface_name.clone(),
            functions,
        })
    }

    fn build_interface(iface: &InterfaceDef) -> BuildResult<HirInterface> {
        let methods = iface
            .methods
            .iter()
            .map(|sig| HirFnSig {
                name: sig.name.clone(),
                generic_params: Self::build_generic_params(&sig.generic_params),
                params: sig
                    .params
                    .iter()
                    .map(|p| HirParam {
                        name: p.name.clone(),
                        ty: p.ty.clone(),
                    })
                    .collect(),
                return_type: sig.return_type.clone(),
            })
            .collect();

        Ok(HirInterface {
            name: iface.name.clone(),
            generic_params: Self::build_generic_params(&iface.generic_params),
            methods,
        })
    }

    fn build_model(
        m: &ModelDef,
        expr_types: &HashMap<usize, Type>,
    ) -> BuildResult<HirModel> {
        // 之前这里、下面的 training_context_required、privacy_eps 三处
        // 各自 `m.functions.iter().find(|f| f.name == "forward")` 扫一遍，
        // n 很小所以性能无所谓，但逻辑分散、改起来容易漏。这里只扫一次，
        // 后面几处都复用同一个 forward。
        let forward = m.functions.iter().find(|f| f.name == "forward");

        // BTreeSet 天然去重 + 有序，省掉手写的 unique_syms.contains 检查
        // 和最后的 sort。
        let mut syms: BTreeSet<String> = BTreeSet::new();
        if let Some(fwd) = forward {
            Self::collect_syms_from_type(&fwd.return_type, &mut syms);
            for param in &fwd.params {
                Self::collect_syms_from_type(&Some(param.ty.clone()), &mut syms);
            }
        }

        // 注：这些 symbolic dims 来自 forward 的张量形状，不是用户写的 <T: Bound>，
        // 天生没有 bounds 概念。这里只是包一层 HirGenericParam 让类型跟其它
        // generic_params 字段保持一致，bounds 恒为空，不代表真的支持约束。
        let generic_params = syms
            .into_iter()
            .map(|name| HirGenericParam {
                name,
                bounds: Vec::new(),
            })
            .collect();

        let fields = m
            .fields
            .iter()
            .map(|f| HirField {
                name: f.name.clone(),
                ty: f.ty.clone(),
            })
            .collect();

        let functions = m
            .functions
            .iter()
            .map(|f| Self::build_fn(f, expr_types, true))
            .collect::<Result<Vec<_>, _>>()?;

        let sensitivity = None;

        // 检查是否需要 TrainingContext（复用上面已经拿到的 forward）
        // 关键修复：改回显式 match，不用 matches! 宏——项目里明确规定不用
        // Rust 的任何宏（包括 matches!）。
        let training_context_required = forward.map_or(false, |fwd| {
            fwd.params.iter().any(|p| match &p.ty {
                Type::Privacy(_, PrivacyTag::Differential { .. }) => true,
                _ => false,
            })
        });

        // 提取隐私预算 eps（同样复用 forward）
        let privacy_eps = forward.and_then(|fwd| {
            fwd.params.iter().find_map(|p| {
                if let Type::Privacy(_, PrivacyTag::Differential { eps, delta: _ }) = &p.ty {
                    Some(eps.clone())
                } else {
                    None
                }
            })
        });

        Ok(HirModel {
            name: m.name.clone(),
            generic_params,
            fields,
            functions,
            sensitivity,
            training_context_required,
            privacy_eps,
        })
    }

    fn build_proto(p: &ProtoDef) -> BuildResult<HirProto> {
        Ok(HirProto {
            name: p.name.clone(),
            variants: p
                .variants
                .iter()
                .map(|v| HirProtoVariant {
                    name: v.name.clone(),
                    ty: v.ty.clone(),
                })
                .collect(),
        })
    }

    fn build_fn(
        f: &FnDef,
        expr_types: &HashMap<usize, Type>,
        in_model: bool,
    ) -> BuildResult<HirFn> {
        let params = f
            .params
            .iter()
            .map(|p| HirParam {
                name: p.name.clone(),
                ty: p.ty.clone(),
            })
            .collect();

        let body = Self::build_block(&f.body, expr_types)?;
        let return_type = f.return_type.clone();
        // 之前这里是 EffectSet::default()，等于所有函数的 effect 恒为
        // false——这是 bug，不是"留给后面单独 pass 算"的占位符（目前
        // 压根没有那样的 pass）。函数级 effect 应该是函数体里所有语句
        // effect 的合并，直接复用 merge_effects_from_block。
        let effects = Self::merge_effects_from_block(&body);
        let sensitivity = None;

        Ok(HirFn {
            name: f.name.clone(),
            generic_params: Self::build_generic_params(&f.generic_params),
            params,
            return_type,
            body,
            effects,
            sensitivity,
            is_forward: f.name == "forward" && in_model,
        })
    }

    fn build_block(block: &Block, expr_types: &HashMap<usize, Type>) -> BuildResult<HirBlock> {
        let mut stmts = Vec::new();
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let(let_stmt) => {
                    let init = Self::build_expr(&let_stmt.init, expr_types)?;
                    stmts.push(HirStmt::Let {
                        name: let_stmt.name.clone(),
                        ty: let_stmt.ty.clone(),
                        init,
                        mutable: let_stmt.mutable,
                        persist: let_stmt.persist,
                        span: Span::default(),
                    });
                }
                Stmt::ExprStmt(expr) => {
                    let expr = Self::build_expr(expr, expr_types)?;
                    stmts.push(HirStmt::Expr {
                        expr,
                        span: Span::default(),
                    });
                }
                Stmt::Return(expr_opt) => {
                    let expr = expr_opt
                        .as_ref()
                        .map(|e| Self::build_expr(e, expr_types))
                        .transpose()?;
                    stmts.push(HirStmt::Return {
                        expr,
                        span: Span::default(),
                    });
                }
                Stmt::While(while_stmt) => {
                    let cond = Self::build_expr(&while_stmt.cond, expr_types)?;
                    let body = Self::build_block(&while_stmt.body, expr_types)?;
                    stmts.push(HirStmt::While {
                        cond,
                        body,
                        span: Span::default(),
                    });
                }
                Stmt::For(for_stmt) => {
                    let iterable = Self::build_expr(&for_stmt.iterable, expr_types)?;
                    let body = Self::build_block(&for_stmt.body, expr_types)?;
                    stmts.push(HirStmt::For {
                        var: for_stmt.var.clone(),
                        iterable,
                        body,
                        span: Span::default(),
                    });
                }
                Stmt::Assign(assign_stmt) => {
                    let target = Self::build_expr(&assign_stmt.target, expr_types)?;
                    let expr = Self::build_expr(&assign_stmt.expr, expr_types)?;
                    stmts.push(HirStmt::Assign {
                        target: Box::new(target),
                        expr,
                        span: Span::default(),
                    });
                }
                Stmt::Loop(loop_stmt) => {
                    let body = Self::build_block(&loop_stmt.body, expr_types)?;
                    stmts.push(HirStmt::Loop {
                        body,
                        span: Span::default(),
                    });
                }
                Stmt::Break(_) => {
                    stmts.push(HirStmt::Break {
                        span: Span::default(),
                    });
                }
                Stmt::UnsafeBlock(unsafe_block) => {
                    let body = Self::build_block(&unsafe_block.body, expr_types)?;
                    stmts.push(HirStmt::UnsafeBlock {
                        kind: unsafe_block.kind.clone(),
                        body,
                        span: Span::default(),
                    });
                }
            }
        }
        Ok(HirBlock {
            stmts,
            span: Span::default(),
        })
    }

    // 把构造 HirExpr 时反复重复的 6 个字段收口成一个函数。privacy_tag
    // 直接从传入的 ty 现取，调用方不用再单独维护一份 privacy_tag 变量。
    // 注意：privacy_tag 必须在 ty 被移入结构体之前算出来，所以这里用
    // let 先算好，不能写成 struct literal 里 `ty, privacy_tag: ty.privacy_tag()`
    // 那种顺序——ty 字段先被移动，后面再借用就编译不过了。
    fn mk_expr(kind: HirExprKind, ty: Type, effects: EffectSet, span: Span) -> HirExpr {
        let privacy_tag = ty.privacy_tag();
        HirExpr {
            kind,
            ty,
            privacy_tag,
            sensitivity: Sensitivity::Unknown,
            effects,
            span,
        }
    }

    fn build_expr(expr: &Expr, expr_types: &HashMap<usize, Type>) -> BuildResult<HirExpr> {
        let ty = expr_types.get(&expr.id).cloned().unwrap_or(Type::I32);
        let mut effects = EffectSet::default();

        let kind = match &expr.kind {
            ExprKind::Literal(lit) => HirExprKind::Literal(lit.clone()),
            ExprKind::Ident(name) => HirExprKind::Ident(name.clone()),
            ExprKind::Sym(name) => HirExprKind::Sym(name.clone()),
            ExprKind::BinaryOp { op, left, right } => {
                let left = Self::build_expr(left, expr_types)?;
                let right = Self::build_expr(right, expr_types)?;
                effects = Self::merge_effects([&left, &right]);
                HirExprKind::BinaryOp {
                    op: op.clone(),
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            ExprKind::Call {
                qualifier,
                func,
                args,
                is_method,
            } => {
                let hir_args = Self::build_call_args(args, expr_types)?;
                effects = Self::merge_effects_from_call_args(&hir_args);
                if func == "print" {
                    effects.has_io = true;
                }
                HirExprKind::Call {
                    qualifier: qualifier.clone(),
                    func: func.clone(),
                    // TODO(sema): 目前 ast::ExprKind::Call 还没有显式泛型实参语法（如
                    // identity::<i32>(1)），先占位空 Vec，等 parser/ast 支持后这里改成
                    // 从 ast 侧透传
                    generic_args: Vec::new(),
                    args: hir_args,
                    is_method: *is_method,
                }
            }
            ExprKind::Block(block) => {
                let hir_block = Self::build_block(block, expr_types)?;
                effects = Self::merge_effects_from_block(&hir_block);
                HirExprKind::Block(hir_block)
            }
            ExprKind::StructInit {
                struct_name,
                fields,
            } => {
                let hir_fields: Vec<(String, HirExpr)> = fields
                    .iter()
                    .map(|(name, e)| {
                        let expr = Self::build_expr(e, expr_types)?;
                        Self::ok((name.clone(), expr))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                effects = Self::merge_effects(hir_fields.iter().map(|(_, e)| e));
                HirExprKind::StructInit {
                    struct_name: struct_name.clone(),
                    generic_args: Vec::new(),
                    fields: hir_fields,
                }
            }
            ExprKind::FieldAccess {
                struct_expr,
                field_name,
            } => {
                let struct_expr = Self::build_expr(struct_expr, expr_types)?;
                effects = struct_expr.effects.clone();
                HirExprKind::FieldAccess {
                    struct_expr: Box::new(struct_expr),
                    field_name: field_name.clone(),
                }
            }
            ExprKind::Range { start, end } => {
                let start = Self::build_expr(start, expr_types)?;
                let end = Self::build_expr(end, expr_types)?;
                effects = Self::merge_effects([&start, &end]);
                HirExprKind::Range {
                    start: Box::new(start),
                    end: Box::new(end),
                }
            }
            ExprKind::EnumVariantAccess {
                enum_name,
                variant_name,
            } => HirExprKind::EnumVariantAccess {
                enum_name: enum_name.clone(),
                variant_name: variant_name.clone(),
            },
            // ===== 新增：枚举变体构造 =====
            ExprKind::EnumVariantConstruction {
                enum_name,
                variant_name,
                args,
            } => {
                let hir_args = Self::build_call_args(args, expr_types)?;
                effects = Self::merge_effects_from_call_args(&hir_args);

                HirExprKind::EnumVariantConstruction {
                    enum_name: enum_name.clone(),
                    // TODO(sema): 同 Call，ast 侧暂无显式泛型实参，先占位空 Vec
                    generic_args: Vec::new(),
                    variant_name: variant_name.clone(),
                    args: hir_args,
                }
            }
            ExprKind::Match(match_expr) => {
                let cond = Self::build_expr(&match_expr.cond, expr_types)?;
                let mut arms = Vec::new();
                for arm in &match_expr.arms {
                    let arm_expr = Self::build_expr(&arm.expr, expr_types)?;
                    arms.push(HirMatchArm {
                        pattern: arm.pattern.clone(),
                        expr: arm_expr,
                    });
                }
                effects = Self::merge_effects(
                    std::iter::once(&cond).chain(arms.iter().map(|arm| &arm.expr)),
                );
                HirExprKind::Match {
                    cond: Box::new(cond),
                    arms,
                }
            }
            ExprKind::Closure { param, body } => {
                let body = Self::build_expr(body, expr_types)?;
                effects = body.effects.clone();
                HirExprKind::Closure {
                    param: param.clone(),
                    body: Box::new(body),
                }
            }
            ExprKind::If {
                kind: if_kind,
                cond,
                then_expr,
                else_expr,
            } => {
                let cond = Self::build_expr(cond, expr_types)?;
                let then_expr = Self::build_expr(then_expr, expr_types)?;
                let else_expr = else_expr
                    .as_ref()
                    .map(|e| Self::build_expr(e, expr_types))
                    .transpose()?
                    .map(Box::new);
                effects = Self::merge_effects(
                    std::iter::once(&cond)
                        .chain(std::iter::once(&then_expr))
                        .chain(else_expr.as_deref()),
                );
                HirExprKind::If {
                    kind: if_kind.clone(),
                    cond: Box::new(cond),
                    then_expr: Box::new(then_expr),
                    else_expr,
                }
            }
            ExprKind::ArrayLiteral(elements) => {
                let hir_elements: Vec<HirExpr> = elements
                    .iter()
                    .map(|e| Self::build_expr(e, expr_types))
                    .collect::<Result<Vec<_>, _>>()?;
                effects = Self::merge_effects(hir_elements.iter());
                HirExprKind::ArrayLiteral(hir_elements)
            }
            ExprKind::UnsafeBlock(unsafe_block) => {
                let body = Self::build_block(&unsafe_block.body, expr_types)?;
                effects = Self::merge_effects_from_block(&body);
                HirExprKind::UnsafeBlock {
                    kind: unsafe_block.kind.clone(),
                    body,
                    span: Span::default(),
                }
            }
            // ===== 新增：一元运算符 =====
            ExprKind::Unary { op, expr } => {
                let inner = Self::build_expr(expr, expr_types)?;
                effects = inner.effects.clone();
                HirExprKind::Unary {
                    op: op.clone(),
                    expr: Box::new(inner),
                }
            }
            // ===== 新增：as 类型转换 =====
            ExprKind::Cast { expr, ty: cast_ty } => {
                let inner = Self::build_expr(expr, expr_types)?;
                effects = inner.effects.clone();
                HirExprKind::Cast {
                    expr: Box::new(inner),
                    ty: cast_ty.clone(),
                }
            }
            // ===== 新增：索引表达式 expr[idx]（比如 bytes[i]）=====
            ExprKind::Index { expr, index } => {
                let base = Self::build_expr(expr, expr_types)?;
                let idx = Self::build_expr(index, expr_types)?;
                effects = Self::merge_effects([&base, &idx]);
                HirExprKind::Index {
                    expr: Box::new(base),
                    index: Box::new(idx),
                }
            }
            // ===== 新增：lack &[T] 空切片字面量——纯编译期常量，effect 为
            // none，跟规范里"b""/lack &[T] 均为编译期常量"这条一致，
            // effects 保持默认（全 false），不用合并任何子表达式的副作用
            // （它本来就没有子表达式）。
            ExprKind::LackSlice(ty) => HirExprKind::LackSlice(ty.clone()),
        };

        Ok(Self::mk_expr(kind, ty, effects, Span::default()))
    }

    fn build_struct(s: &StructDef) -> BuildResult<HirStruct> {
        let generic_params = Self::build_generic_params(&s.generic_params);

        Ok(HirStruct {
            name: s.name.clone(),
            generic_params,
            fields: s
                .fields
                .iter()
                .map(|f| HirField {
                    name: f.name.clone(),
                    ty: f.ty.clone(),
                })
                .collect(),
        })
    }

    fn build_enum(e: &EnumDef) -> BuildResult<HirEnum> {
        let generic_params = Self::build_generic_params(&e.generic_params);

        Ok(HirEnum {
            name: e.name.clone(),
            generic_params,
            variants: e
                .variants
                .iter()
                .map(|v| HirEnumVariant {
                    name: v.name.clone(),
                    ty: v.ty.clone(),
                })
                .collect(),
        })
    }

    fn build_const(c: &ConstDef, expr_types: &HashMap<usize, Type>) -> BuildResult<HirConst> {
        Ok(HirConst {
            name: c.name.clone(),
            ty: c.ty.clone(),
            value: Self::build_expr(&c.value, expr_types)?,
        })
    }

    fn collect_syms_from_type(ty_opt: &Option<Type>, syms: &mut BTreeSet<String>) {
        if let Some(ty) = ty_opt {
            Self::collect_syms_from_type_inner(ty, syms);
        }
    }

    fn collect_syms_from_type_inner(ty: &Type, syms: &mut BTreeSet<String>) {
        match ty {
            Type::Tensor { shape, .. } => {
                for dim in shape {
                    if let ShapeDim::Sym(s) = dim {
                        syms.insert(s.clone());
                    }
                }
            }
            Type::Privacy(inner, _) => Self::collect_syms_from_type_inner(inner, syms),
            _ => {}
        }
    }
}