// mir_builder.rs
use crate::ast::{Pattern, Type};
use crate::hir::*;
use crate::mir::*;
use std::collections::HashMap;
use crate::intrinsic::{IntrinsicFn, IntrinsicConst, get_intrinsic, get_constant};

// ===== 跨函数共享的只读上下文（build() 里构建一次，每个函数复用） =====
struct SharedContext {
    // struct_name -> (field_name -> field_ty)，FieldAccess 查真实字段
    // 类型用，不再靠猜。
    struct_fields: HashMap<String, HashMap<String, Type>>,
    // variant_name -> enum_name（要求全局唯一）。裸 Ok/Err/Some/None 这类
    // 不带 :: 前缀的写法，语法上跟普通函数调用（HirExprKind::Call）长得
    // 一模一样，sema.rs 那边靠"在所有已注册枚举里找恰好一个同名变体"
    // 识别出来，但那个识别结果只体现在类型检查上，没有改写 HIR 节点
    // 本身——MIR 构建这里得重新做一遍同样的查找，才能正确区分
    // "Err(())" 这种裸枚举变体构造和真正的函数调用。
    variant_to_enum: HashMap<String, String>,
    variant_indices: HashMap<(String, String), usize>,
    // 关键新增：(enum_name, variant_name) -> 这个变体自己声明的 payload
    // 类型。EnumVariantWithBinding 模式（`Ok(v) => ...` 里的 v）要把
    // payload 解出来绑定成一个新的 MirLocal，而 MirLocal.ty 是必填
    // 字段——不能瞎猜一个类型糊弄过去，也没法从 Switch 那边反推出来
    // （Switch 只留了 i64 下标，早就不知道原来的 payload 长什么样了），
    // 只能在这里从 hir.enums 的原始声明里查。
    variant_payload_types: HashMap<(String, String), Type>,
}

pub struct MirBuilder {
    locals: Vec<MirLocal>,
    blocks: Vec<MirBlock>,
    current_block: usize,
    scope: Vec<HashMap<String, usize>>, // 变量名 -> local id
    scope_vars: Vec<Vec<usize>>,
    unsafe_depth: usize,
    in_forward: bool,
    ssa_versions: HashMap<usize, u32>,
    // 关键修复（找回上一轮被回退掉的东西）：这一版是从更早的快照分支
    // 出来重新改的，上一轮为了配合真正的 Drop 语义加的 `moved` 追踪
    // （连带 pop_scope/Return 那两处修复）整个不见了，pop_scope 现在
    // 又是无条件对作用域里的每个变量插 Drop——回到了"对已经被移动走
    // （哪怕只是部分移动）的值重复调用 drop()，生成的 Rust 编译不过"
    // 这个问题。理由和之前完全一样，不重复展开，直接照抄那一轮的实现。
    moved: std::collections::HashSet<SsaLocal>,
}

impl MirBuilder {
    pub fn build(hir: &HirProgram) -> Result<MirProgram, String> {
        let struct_fields: HashMap<String, HashMap<String, Type>> = hir
            .structs
            .iter()
            .map(|s| {
                let fields = s.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
                (s.name.clone(), fields)
            })
            .collect();

        let mut variant_to_enum: HashMap<String, String> = HashMap::new();
        let mut ambiguous: std::collections::HashSet<String> = std::collections::HashSet::new();
        for e in &hir.enums {
            for v in &e.variants {
                if variant_to_enum.contains_key(&v.name) {
                    ambiguous.insert(v.name.clone());
                } else {
                    variant_to_enum.insert(v.name.clone(), e.name.clone());
                }
            }
        }
        for name in &ambiguous {
            variant_to_enum.remove(name);
        }

        let mut variant_indices: HashMap<(String, String), usize> = HashMap::new();
        let mut variant_payload_types: HashMap<(String, String), Type> = HashMap::new();
        for e in &hir.enums {
            for (idx, v) in e.variants.iter().enumerate() {
                variant_indices.insert((e.name.clone(), v.name.clone()), idx);
                if let Some(ty) = &v.ty {
                    variant_payload_types.insert((e.name.clone(), v.name.clone()), ty.clone());
                }
            }
        }

        let shared = SharedContext {
            struct_fields,
            variant_to_enum,
            variant_indices,
            variant_payload_types,
        };

        let mut fns = Vec::new();
        // 顶层函数
        for f in &hir.fns {
            fns.push(Self::build_fn(f, &shared)?);
        }
        // model 里的方法（forward 等）
        for m in &hir.models {
            for f in &m.functions {
                fns.push(Self::build_fn(f, &shared)?);
            }
        }
        // 关键修复：之前完全没遍历 hir.impls——Vec/String/Rational 等等
        // 标准库里几乎所有方法都定义在 implement 块里，只在 hir.impls
        // 而不在 hir.fns 里。不补上这段，标准库的全部方法在 MIR 这层会
        // 直接消失。
        for imp in &hir.impls {
            for f in &imp.functions {
                fns.push(Self::build_fn(f, &shared)?);
            }
        }

        let structs = hir
            .structs
            .iter()
            .map(|s| MirStruct {
                name: s.name.clone(),
                generic_params: s.generic_params.clone(),
                fields: s.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect(),
            })
            .collect();

        let enums = hir
            .enums
            .iter()
            .map(|e| MirEnum {
                name: e.name.clone(),
                generic_params: e.generic_params.clone(),
                variants: e.variants.iter().map(|v| (v.name.clone(), v.ty.clone())).collect(),
            })
            .collect();

        // 内建函数使用情况：intrinsic.rs 的注册表现在已经做好了，这里
        // 汇总的是每个函数构建时各自收集到的真实 IntrinsicFn（不再是
        // 权宜之计）。
        let mut intrinsics_used: Vec<IntrinsicFn> = fns
            .iter()
            .flat_map(|f| Self::collect_intrinsics_in_body(&f.body))
            .collect();
        intrinsics_used.sort();
        intrinsics_used.dedup();

        // TODO: consts / protos / interfaces 目前没有对应的 Mir* 结构，
        // 先只覆盖 fns/structs/enums(+ 现在补上的 impls/models) 把主干
        // 打通。
        Ok(MirProgram { structs, enums, fns, intrinsics_used })
    }

    fn collect_intrinsics_in_body(body: &MirBody) -> Vec<IntrinsicFn> {
        let mut out = Vec::new();
        for block in &body.blocks {
            for stmt in &block.stmts {
                let rv = match stmt {
                    MirStmt::Assign { value, .. } => Some(value),
                    MirStmt::ExprStmt(value) => Some(value),
                    _ => None,
                };
                if let Some(MirRvalue::Call { intrinsic_name: Some(name), .. }) = rv {
                    out.push(*name);
                }
            }
        }
        out
    }

    fn build_fn(f: &HirFn, shared: &SharedContext) -> Result<MirFn, String> {
        let mut builder = MirBuilder {
            locals: Vec::new(),
            blocks: Vec::new(),
            current_block: 0,
            scope: vec![HashMap::new()],
            scope_vars: vec![Vec::new()],
            unsafe_depth: 0,
            in_forward: f.is_forward,
            ssa_versions: HashMap::new(), 
            moved: std::collections::HashSet::new(),
        };
        builder.new_block(); // 入口块，id = 0（struct 里 current_block 已经是 0，不用再赋一次）

        for param in &f.params {
            let id = builder.new_param_local(param.name.clone(), param.ty.clone());
            builder.scope.last_mut().unwrap().insert(param.name.clone(), id);
        }

        let ret = builder.build_block(&f.body, shared)?;
        // 函数体最后一个值需要作为 Return 的值，除非最后一句已经是显式
        // return（这种情况下当前块的终止器已经被 build_stmt 设置好了，
        // 不能再覆盖——用 Unreachable 占位判断"这个块是不是已经有真正
        // 的终止器"）。
        // 关键修复：如果函数签名的 return_type 是 Type::Never（这个函数
        // 本来就声明"不会正常返回"，比如 fn abort() -> never { loop {} }），
        // 走到这里说明函数体正常"掉出了"最后一句、没有被任何显式终止器
        // 拦下——按 Never 的语义这块根本不该有 Return 边，之前没有真正
        // 的 Type::Never 时，这里只能无条件塞一个 Return(ret) 权充，现在
        // 改成如实标成 Unreachable。sema 应该已经保证 Never 函数的每条
        // 路径都会发散，这里只是让 MIR 层的终止器诚实地反映这条约定，
        // 不负责重新验证它。
        if builder.current_terminator_is_placeholder() {
            let term = if matches!(f.return_type, Some(Type::Never)) {
                MirTerminator::Unreachable
            } else {
                MirTerminator::Return(ret)
            };
            builder.set_terminator(term);
        }

        Ok(MirFn {
            name: f.name.clone(),
            generic_params: f.generic_params.clone(),
            params: f.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
            return_type: f.return_type.clone(),
            body: MirBody { locals: builder.locals, blocks: builder.blocks },
            effect_set: f.effects.clone(),
        })
    }

    // -------- 局部变量 --------
    fn new_local(&mut self, name: Option<String>, ty: Type, mutable: bool, add_to_scope: bool) -> usize {
        let id = self.locals.len();
        self.locals.push(MirLocal { id, name, ty, mutable, persist: false, is_param: false });
        if add_to_scope {
            self.scope_vars.last_mut().unwrap().push(id);
        }
        id
    }

    // 关键新增：专门给函数参数用——is_param: true，codegen.rs 靠这个
    // 字段知道"这个 local 不用重新 let 声明，Rust 函数签名里已经有
    // 同名的绑定了"。参数永远不是 persist（persist 是给 model 块里
    // `persist let`/`persist var` 用的，跟参数是两回事），也不需要
    // mutable（函数体内要重新赋值的话，语言层面应该是 `let mut x = 参数`
    // 这种显式重绑定，走的是普通 new_local，不是这里）。
    fn new_param_local(&mut self, name: String, ty: Type) -> usize {
        let id = self.locals.len();
        self.locals.push(MirLocal { id, name: Some(name), ty, mutable: false, persist: false, is_param: true });
        id
    }

    fn new_persist_local(&mut self, name: Option<String>, ty: Type, mutable: bool) -> usize {
        let id = self.locals.len();
        self.locals.push(MirLocal { id, name, ty, mutable, persist: true, is_param: false });
        self.scope_vars.last_mut().unwrap().push(id);
        id
    }

    fn new_version(&mut self, base_id: usize) -> u32 {
        let ver = self.ssa_versions.get(&base_id).copied().unwrap_or(0) + 1;
        self.ssa_versions.insert(base_id, ver);
        ver
    }

    fn new_temp(&mut self, ty: Type) -> usize {
        self.new_local(None, ty, false, true)
    }

    fn current_ssa(&self, base_id: usize) -> SsaLocal {
        let version = self.ssa_versions.get(&base_id).copied().unwrap_or(0);
        SsaLocal { base_id, version }
    }

    // 关键新增：Pattern::IntLiteral 只存了一个裸 i64（这是 ast.rs 里
    // Pattern 自己的限制，还没跟着这一轮 Literal 拆分成按位宽/符号
    // 区分的一堆变体一起升级——也就是说目前没法用字面量模式匹配超出
    // i64 范围的 i128/u128 值，这是个已知的、比这次修复范围更大的
    // 缺口，这里先不动 ast.rs，只保证"i64 范围内的值，按 discr 的具体
    // 类型转换成正确的 Literal 变体"这件事是对的）。
    fn int_literal_for_type(v: i64, ty: &Type) -> Result<Literal, String> {
        Ok(match ty {
            Type::I8 => Literal::Int8(v as i8),
            Type::I16 => Literal::Int16(v as i16),
            Type::I32 => Literal::Int32(v as i32),
            Type::I64 => Literal::Int64(v),
            Type::I128 => Literal::Int128(v as i128),
            Type::U8 => Literal::UInt8(v as u8),
            Type::U16 => Literal::UInt16(v as u16),
            Type::U32 => Literal::UInt32(v as u32),
            Type::U64 => Literal::UInt64(v as u64),
            Type::U128 => Literal::UInt128(v as u128),
            other => return Err(format!(
                "match 条件是整数类型，但字面量模式配的类型是 {:?}——不是任何已知的整数类型，\
                 这本该在 sema 阶段就被拦下",
                other
            )),
        })
    }

    // -------- 作用域 --------
    fn push_scope(&mut self) {
        self.scope.push(HashMap::new());
        self.scope_vars.push(Vec::new());
    }
    fn pop_scope(&mut self) {
        if let Some(ids) = self.scope_vars.pop() {
            for id in ids {
                let ssa = self.current_ssa(id);
                // 关键修复（找回上一轮的修复）：不能对作用域里的每个
                // 变量无条件插 Drop——如果这个变量的当前版本已经在这个
                // 作用域内被"消费"过（完整读取过一次，或者被当成
                // Field/Index/EnumPayload 的 base 部分移动过），再补一条
                // Drop 就是对一个已经移动走的值重复使用，生成的 Rust 会
                // 是 "use of (partially) moved value"，编译不过。典型
                // 场景：`match p { Point { x, y } => x }`——x 被直接当成
                // 匹配结果读出去了，不能再 Drop 一次；`Ok(v) => v`、块尾
                // 直接返回一个局部变量，都是同一类问题。见
                // MirBuilder.moved 字段的说明，以及下面几处往里登记的
                // 地方。
                if !self.moved.contains(&ssa) {
                    self.push_stmt(MirStmt::Drop { place: MirPlace::Ssa(ssa) });
                }
            }
        }
        self.scope.pop();
    }
    fn bind(&mut self, name: String, id: usize) {
        self.scope.last_mut().unwrap().insert(name, id);
    }
    fn lookup(&self, name: &str) -> Option<usize> {
        for scope in self.scope.iter().rev() {
            if let Some(id) = scope.get(name) {
                return Some(*id);
            }
        }
        None
    }

    // -------- 基本块 --------
    // 关键设计：块在用到之前就先创建好（占位终止器是 Unreachable），
    // 之后随时可以用 id 引用它、往里面塞语句，最后再补上真正的终止器。
    // 这是为了支持 if/while 这类需要"提前知道 then/else 块的 id 才能
    // 设置当前块的跳转目标"的控制流——不这样做的话，构建顺序会陷入
    // "先有鸡还是先有蛋"的死结。
    fn new_block(&mut self) -> usize {
        let id = self.blocks.len();
        self.blocks.push(MirBlock {
            id,
            stmts: Vec::new(),
            terminator: MirTerminator::Unreachable,
        });
        id
    }

    fn switch_to_block(&mut self, id: usize) {
        self.current_block = id;
    }

    fn push_stmt(&mut self, stmt: MirStmt) {
        self.blocks[self.current_block].stmts.push(stmt);
    }

    fn set_terminator(&mut self, term: MirTerminator) {
        self.blocks[self.current_block].terminator = term;
    }

    fn current_terminator_is_placeholder(&self) -> bool {
        matches!(self.blocks[self.current_block].terminator, MirTerminator::Unreachable)
    }

    // -------- Block / Stmt --------
    fn build_block(&mut self, block: &HirBlock, shared: &SharedContext) -> Result<Option<MirOperand>, String> {
        self.push_scope();
        let mut last: Option<MirOperand> = None;
        for stmt in &block.stmts {
            last = self.build_stmt(stmt, shared)?;
        }
        self.pop_scope();
        Ok(last)
    }

    fn build_stmt(&mut self, stmt: &HirStmt, shared: &SharedContext) -> Result<Option<MirOperand>, String> {
        match stmt {
            HirStmt::Let { name, ty, init, mutable, persist, .. } => {
                let value = self.build_expr(init, shared)?;
                let local_ty = ty.clone().unwrap_or_else(|| init.ty.clone());
                let id = if *persist {
                    self.new_persist_local(Some(name.clone()), local_ty, *mutable)
                } else {
                    self.new_local(Some(name.clone()), local_ty, *mutable, true)
                };
                self.bind(name.clone(), id);
                // 递增版本号（第一次赋值，版本从 0 → 1）
                let version = self.ssa_versions.get(&id).copied().unwrap_or(0);
                let new_version = version + 1;
                self.ssa_versions.insert(id, new_version);
                let dest = MirPlace::Ssa(SsaLocal { base_id: id, version: new_version });
                self.push_stmt(MirStmt::Assign {
                    dest,
                    value: MirRvalue::Use(value),
                });
                Ok(None)
            }
            HirStmt::Expr { expr, .. } => {
                let value = self.build_expr_rvalue(expr, shared)?;
                self.push_stmt(MirStmt::ExprStmt(value));
                // 关键修复：这条语句自身的类型是 Type::Never（比如单独
                // 一句 panic()，不是 `return`，但一样不会把控制流交回
                // 下一条语句）时，当前块在这里其实已经终止了——跟下面
                // HirStmt::Return 分支"发一个新块承接后面的死代码"是同
                // 一个道理，不这样做的话，写在这句后面的语句会被继续塞
                // 进一个实际上已经不可达的块里，跟真实控制流对不上。
                // 之前没有真正的 Type::Never 时这里根本判断不出来，只能
                // 放任不管，这是当初的权宜之计之一，现在补上。
                if matches!(expr.ty, Type::Never) {
                    self.set_terminator(MirTerminator::Unreachable);
                    let next = self.new_block();
                    self.switch_to_block(next);
                }
                Ok(None)
            }
            HirStmt::Return { expr, .. } => {
                // 先计算返回值——build_expr 如果读到的是某个局部变量，
                // 会顺带把它标进 self.moved（见下面 Drop 循环为什么要
                // 查这张表），这个顺序不能反。
                let operand = expr.as_ref().map(|e| self.build_expr(e, shared)).transpose()?;

                // 关键修复（找回上一轮的修复）：原来遍历的是
                // `self.locals`——整个函数构建过程中创建过的所有局部
                // 变量的历史记录，包含早就走出作用域、已经被对应的
                // pop_scope 释放过一次的变量，对同一个变量重复插 Drop
                // 会导致 "use of moved value"。应该只 Drop"当前仍然开着
                // 的那些作用域"里登记的变量（self.scope_vars，从内层到
                // 外层遍历），并且跳过已经被消费过的变量（比如
                // `return x;` 直接返回一个局部变量，x 已经被上面
                // build_expr 标记过）。函数参数从来不会被塞进
                // scope_vars（new_param_local 不会调用 scope_vars 相关
                // 的 push），这里天然不会碰到它们，不需要再单独判断
                // `!local.is_param`。
                // 关键修复：原来这里一边 `self.scope_vars.iter().rev()`
                // 对 self 的不可变借用，一边在循环体里调用
                // `self.push_stmt(...)`（需要 &mut self）——push_stmt 是
                // 方法调用，对借用检查器来说是不透明的黑盒，哪怕
                // scope_vars 和存语句那块字段在结构上互不相干，借用
                // 检查器也没法看穿方法内部只碰了另一个字段，只能保守地
                // 认为整个 `*self` 被借着，判成 E0502（不可变借用还活着
                // 的时候又要可变借用）。
                //
                // 做法是把"读哪些变量该 Drop"和"真正写 Drop 语句"这两步
                // 拆开：先只读地把要 Drop 的 SsaLocal 收集进一个独立的
                // Vec（这一步只碰 scope_vars/moved，用的都是 &self），
                // 读完这一轮不可变借用自然结束；再单独一轮遍历调用
                // push_stmt，这时候已经没有任何借用跟它重叠了。
                let mut to_drop = Vec::new();
                for scope_ids in self.scope_vars.iter().rev() {
                    for &id in scope_ids.iter().rev() {
                        let ssa = self.current_ssa(id);
                        if !self.moved.contains(&ssa) {
                            to_drop.push(ssa);
                        }
                    }
                }
                for ssa in to_drop {
                    self.push_stmt(MirStmt::Drop { place: MirPlace::Ssa(ssa) });
                }

                self.set_terminator(MirTerminator::Return(operand));
                let next = self.new_block();
                self.switch_to_block(next);
                Ok(None)
            }
            HirStmt::Assign { target, expr, .. } => {
                let place = self.build_place(target, shared)?;
                // 如果目标是 Ssa，则递增版本；否则（字段/索引等）保留原样
                let dest = match place {
                    MirPlace::Ssa(ssa) => {
                        let new_ver = self.new_version(ssa.base_id);
                        MirPlace::Ssa(SsaLocal { base_id: ssa.base_id, version: new_ver })
                    }
                    _ => place,
                };
                let value = self.build_expr_rvalue(expr, shared)?;
                self.push_stmt(MirStmt::Assign { dest, value });
                Ok(None)
            }
            HirStmt::While { cond, body, .. } => {
                let cond_block = self.new_block();
                let body_block = self.new_block();
                let end_block = self.new_block();

                self.set_terminator(MirTerminator::Goto(cond_block));

                self.switch_to_block(cond_block);
                let cond_operand = self.build_expr(cond, shared)?;
                self.set_terminator(MirTerminator::If {
                    cond: cond_operand,
                    then_block: body_block,
                    else_block: end_block,
                });

                self.switch_to_block(body_block);
                self.build_block(body, shared)?;
                if self.current_terminator_is_placeholder() {
                    self.set_terminator(MirTerminator::Goto(cond_block));
                }

                self.switch_to_block(end_block);
                Ok(None)
            }
            HirStmt::Loop { body, .. } => {
                let body_block = self.new_block();
                let end_block = self.new_block();

                self.set_terminator(MirTerminator::Goto(body_block));
                self.switch_to_block(body_block);
                self.build_block(body, shared)?;
                if self.current_terminator_is_placeholder() {
                    self.set_terminator(MirTerminator::Goto(body_block));
                }

                self.switch_to_block(end_block);
                Ok(None)
            }
            HirStmt::Break { .. } => {
                // TODO: 需要一个循环栈（跟 While/Loop 配合）才能知道
                // "break 该跳到哪个 end_block"。这一版先把 While/Loop 的
                // 主干打通，循环栈跟 Break/Continue 一起放下一轮——
                // Continue 目前语言里也还没有对应的语句节点（ast.rs/
                // hir.rs 都没有 Continue 变体，parser.rs 也没接语法），
                // 两个一起处理更合适。
                Err("MIR lowering for Break: 循环栈还没接上，下一轮跟 Continue 一起做".to_string())
            }
            HirStmt::For { .. } => {
                // for 循环的降维依赖 Iterator 协议，按流水线设计应该由
                // elaborate.rs 在进入 MIR 构建之前展开成 while + 协议调用。
                // elaborate.rs 目前是空壳，这里先给出清晰的报错而不是
                // panic，方便定位。
                Err("HirStmt::For 不该走到 mir_builder 这一层——应由 elaborate.rs 先展开成 while，但 elaborate.rs 目前还是空实现".to_string())
            }
            HirStmt::UnsafeBlock { body, .. } => {
                self.unsafe_depth += 1;
                self.push_stmt(MirStmt::EffectCheck { effect: "unsafe".to_string() });
                let result = self.build_block(body, shared)?;
                self.unsafe_depth -= 1;
                // 注意：UnsafeBlock 本身是一个语句，它不能产生一个 MirOperand 结果，
                // 所以直接返回 result（可能为 None）
                Ok(result)
            }
        }
    }

    // -------- Place（左值） --------
    fn build_place(&mut self, expr: &HirExpr, shared: &SharedContext) -> Result<MirPlace, String> {
        match &expr.kind {
            HirExprKind::Ident(name) => {
                let id = self.lookup(name).ok_or_else(|| format!("undefined variable `{}`", name))?;
                Ok(MirPlace::Ssa(self.current_ssa(id)))
            }
            HirExprKind::FieldAccess { struct_expr, field_name } => {
                let base = self.build_place(struct_expr, shared)?;
                Ok(MirPlace::Field { base: Box::new(base), field: field_name.clone() })
            }
            HirExprKind::Index { expr: base, index } => {
                let base_place = self.build_place(base, shared)?;
                let index_operand = self.build_expr(index, shared)?;
                Ok(MirPlace::Index { base: Box::new(base_place), index: Box::new(index_operand) })
            }
            _ => Err(format!(
                "internal error: {:?} is not a valid assignment target \
                 (sema.rs 的 is_assignable 应该已经拦住了这种情况，走到这里说明两边检查不一致)",
                expr.kind
            )),
        }
    }

    /// 从 FieldAccess 的 struct_expr 自身携带的类型信息（sema 阶段已经
    /// 解析好），查这个结构体真正定义里的哪个字段名（目前 build_place
    /// 没有用到这个查找结果——MirPlace::Field 只存字段名字符串，类型
    /// 由 codegen 需要时再去 MirProgram.structs 里查，不在 Place 上
    /// 冗余缓存一份，避免跟结构体定义各说各话）。这个函数先保留，等
    /// codegen 真正需要"构建期就确定字段类型"的场景时再启用。
    #[allow(dead_code)]
    fn lookup_field_type(&self, struct_expr: &HirExpr, field_name: &str, shared: &SharedContext) -> Type {
        let struct_name = match &struct_expr.ty {
            Type::Struct(name) => name.clone(),
            Type::Generic(name, _) => name.clone(),
            Type::Ref { inner, .. } => match inner.as_ref() {
                Type::Struct(name) => name.clone(),
                Type::Generic(name, _) => name.clone(),
                _ => return Type::Unit,
            },
            _ => return Type::Unit,
        };
        shared
            .struct_fields
            .get(&struct_name)
            .and_then(|fields| fields.get(field_name))
            .cloned()
            .unwrap_or(Type::Unit)
    }

    // -------- Expr --------
    // build_expr：求值一个表达式，结果materialize 成一个 MirOperand
    // （字面量直接返回 Constant，否则落进临时变量返回 Copy/Move）。
    fn build_expr(&mut self, expr: &HirExpr, shared: &SharedContext) -> Result<MirOperand, String> {
        if let HirExprKind::Literal(lit) = &expr.kind {
            return Ok(MirOperand::Constant(lit.clone()));
        }
        if let HirExprKind::Ident(name) = &expr.kind {
            let id = self.lookup(name).ok_or_else(|| format!("undefined variable `{}`", name))?;
            let ssa = self.current_ssa(id);
            // 关键修复（找回上一轮的修复）：读取一个已有变量统一走
            // Move，就要记下来——见 MirBuilder.moved 字段和 pop_scope
            // 的说明，后面给作用域补 Drop 的时候要跳过它。
            self.moved.insert(ssa);
            return Ok(MirOperand::Move(MirPlace::Ssa(ssa)));
        }
        let rvalue = self.build_expr_rvalue(expr, shared)?;
        // 已经是 Use(operand) 的情况，直接展开，不用画蛇添足再包一层
        // 临时变量。
        if let MirRvalue::Use(operand) = rvalue {
            return Ok(operand);
        }
        let temp = self.new_temp(expr.ty.clone());
        let version = self.ssa_versions.get(&temp).copied().unwrap_or(0);
        let new_version = version + 1;
        self.ssa_versions.insert(temp, new_version);
        let dest = MirPlace::Ssa(SsaLocal { base_id: temp, version: new_version });
        self.push_stmt(MirStmt::Assign { dest, value: rvalue });
        Ok(MirOperand::Move(MirPlace::Ssa(SsaLocal { base_id: temp, version: new_version })))
    }

    // build_expr_rvalue：跟 build_expr 的区别是不强制把结果落进临时
    // 变量——调用方（比如 Stmt::Let/Stmt::Expr）自己决定怎么处理这个
    // Rvalue（要么自己 Assign 进一个具名变量，要么当 ExprStmt 直接丢弃
    // 结果）。build_expr 内部对非字面量/非标识符的情况也是靠它算出
    // Rvalue，再包一层临时变量。
    fn build_expr_rvalue(&mut self, expr: &HirExpr, shared: &SharedContext) -> Result<MirRvalue, String> {
        match &expr.kind {
            HirExprKind::Literal(lit) => Ok(MirRvalue::Use(MirOperand::Constant(lit.clone()))),
            HirExprKind::Ident(name) => {
                let id = self.lookup(name).ok_or_else(|| format!("undefined variable `{}`", name))?;
                let ssa = self.current_ssa(id);
                // 关键修复（找回上一轮的修复）：同 build_expr 里那处。
                self.moved.insert(ssa);
                Ok(MirRvalue::Use(MirOperand::Move(MirPlace::Ssa(ssa))))
            }
            HirExprKind::Sym(name) => {
                // Sym 目前当成一个不可变的具名静态引用处理，精确的
                // 编译期符号求解语义留给 calc.rs/simplify.rs 之后接手。
                Ok(MirRvalue::Use(MirOperand::Move(MirPlace::Static(name.clone()))))
            }
            HirExprKind::BinaryOp { op, left, right } => {
                let l = self.build_expr(left, shared)?;
                let r = self.build_expr(right, shared)?;
                Ok(MirRvalue::BinaryOp(op.clone(), l, r))
            }
            HirExprKind::Unary { op, expr: inner } => {
                let operand = self.build_expr(inner, shared)?;
                Ok(MirRvalue::UnaryOp(op.clone(), operand))
            }
            HirExprKind::Cast { expr: inner, ty } => {
                let operand = self.build_expr(inner, shared)?;
                Ok(MirRvalue::Cast(operand, ty.clone()))
            }
            HirExprKind::FieldAccess { .. } | HirExprKind::Index { .. } => {
                let place = self.build_place(expr, shared)?;
                Ok(MirRvalue::Use(MirOperand::Move(place)))
            }
            HirExprKind::Call { qualifier, func, generic_args, args, is_method } => {
                // 1) 裸枚举变体构造（Ok/Err/Some/None 这类不带前缀的
                //    写法）——见 SharedContext.variant_to_enum 的注释。
                if qualifier.is_none() && !*is_method {
                    if let Some(enum_name) = shared.variant_to_enum.get(func).cloned() {
                        let mut mir_args = Vec::new();
                        for a in args {
                            mir_args.push(self.build_call_arg(a, shared)?);
                        }
                        // 关键修复：这条路径处理的正是最常见的写法——
                        // `Ok(x)`/`Some(x)` 这种裸调用——而不是走
                        // HirExprKind::EnumVariantConstruction 那条限定
                        // 路径。generic_args 本来就是 HirExprKind::Call
                        // 解构出来的字段，之前这里直接没传，是最容易
                        // 漏掉、也是实际最常触发的一个丢数据点。
                        return Ok(MirRvalue::EnumVariantConstruction {
                            enum_name,
                            generic_args: generic_args.clone(),
                            variant_name: func.clone(),
                            args: mir_args,
                        });
                    }
                }

                // 2) 方法调用：第一个参数是 receiver，其余才是真正的实参。
                if *is_method {
                    if args.is_empty() {
                        return Err("method call requires a receiver".to_string());
                    }
                    let receiver = self.build_call_arg(&args[0], shared)?;
                    let mut mir_args = Vec::new();
                    for a in &args[1..] {
                        mir_args.push(self.build_call_arg(a, shared)?);
                    }
                    return Ok(MirRvalue::MethodCall {
                        receiver,
                        method: func.clone(),
                        args: mir_args,
                        generic_args: generic_args.clone(),
                    });
                }

                // 3) 内建关联常量（比如 i128::MAX）：这些在 intrinsic.rs
                //    里注册在 get_constant，不是 get_intrinsic——
                //    get_intrinsic 特意把常量排除在外（见 intrinsic.rs
                //    里 IntrinsicFn/IntrinsicConst 是两个不同枚举的
                //    注释）。不在这里单独拦一道的话，下面第 4 步的
                //    IntrinsicFn::from_str(func) 对这类名字必然返回
                //    None，会一路落进最后"当成普通函数调用"的通用分支，
                //    生成出 `MAX()` 这种把常量当零参函数调用的假代码
                //    ——常量不该走 MirRvalue::Call 这条路，MirPlace::Static
                //    才是它本该落的地方（这个变体当初就是为 i128::MAX
                //    这类常量留的，见 mir.rs 里 MirPlace::Static 的注释）。
                //    关键修复：这里原来错拿 IntrinsicFn::from_str 去解析
                //    "i128::MAX" 这种常量名——IntrinsicFn::from_str 只认
                //    "print"/"panic"/"linear" 这类函数名，永远不可能匹配
                //    上常量名，这一步等于永远走不进去；就算侥幸改成能
                //    匹配，解出来的也是 IntrinsicFn，喂给要 IntrinsicConst
                //    的 get_constant 类型也对不上。IntrinsicFn（函数/图域
                //    算子）和 IntrinsicConst（关联常量）是 intrinsic.rs
                //    里两个独立的枚举，各管各的 from_str，不能混用。
                if qualifier.is_none() && !*is_method && args.is_empty() {
                    if let Some(name) = IntrinsicConst::from_str(func) {
                        if let Some(constant) = get_constant(name) {
                            return Ok(MirRvalue::Use(MirOperand::Move(MirPlace::Static(
                                constant.name.to_string(),
                            ))));
                        }
                    }
                }

                // 4) 真正的内建/固有函数：查 intrinsic.rs 的注册表。
                // 关键修复：原来这里调用的 lookup_by_str(func) 返回的是
                // Option<&'static Intrinsic>（见 intrinsic.rs），根本不是
                // 一个 (name, intrinsic) 二元组——但下面这行硬要
                // `if let Some((name, intrinsic)) = lookup_by_str(func)`
                // 去解构，类型对不上，编译不过（E0308）。而且 lookup_by_str
                // 本身就是 intrinsic.rs 里注释写明"保留旧接口，方便
                // mir_builder 逐步迁移"的兼容性过渡函数——现在直接改成
                // 用 IntrinsicFn::from_str 拿到 name，再用 get_intrinsic
                // 查真正的元数据，两步拼出原来想要的 (name, intrinsic)，
                // 这个过渡接口已经没有存在的必要，intrinsic.rs 里那份
                // 定义也一并删掉了。
                let (is_intrinsic, intrinsic_name) = if qualifier.is_none() && !*is_method {
                    let found = IntrinsicFn::from_str(func)
                        .and_then(|name| get_intrinsic(name).map(|intrinsic| (name, intrinsic)));
                    if let Some((name, intrinsic)) = found {
                        if self.in_forward && !intrinsic.allowed_in_model {
                            return Err(format!(
                                "error[MD001]: side-effect `{}` is not allowed in model block (forward method)",
                                func
                            ));
                        }
        
                        if intrinsic.requires_unsafe && self.unsafe_depth == 0 {
                            return Err(format!(
                                "call to unsafe intrinsic `{}` requires an `unsafe` block or `verify unsafe`",
                                func
                            ));
                        }

                        // 关键新增：交叉校验——sema 给这个调用表达式算出来
                        // 的 expr.ty 和 intrinsic 注册表自己声明的签名，
                        // 对"这个调用是否发散"这件事理论上必须给出一致的
                        // 答案（比如 panic() 的 expr.ty 应该是 Type::Never，
                        // 跟 Intrinsic::diverges() 一致）。只在 debug 构建
                        // 里检查，正常运行零开销；一旦以后 sema 或
                        // intrinsic.rs 哪边单独改了、两边判断标准分了叉，
                        // 这里能在生成出有问题的 MIR 之前就炸出来，而不是
                        // 留到 codegen 生成出编译不过（或者更糟、能编译但
                        // 语义不对）的 Rust 代码才发现。
                        debug_assert_eq!(
                            matches!(expr.ty, Type::Never),
                            intrinsic.diverges(),
                            "intrinsic `{}` 的签名声明的发散性跟 sema 算出的表达式类型对不上",
                            func
                        );

                        (true, Some(name))
                    } else {
                        (false, None)
                    }
                } else {
                    (false, None)
                };

                let full_name = match qualifier {
                    Some(q) => format!("{}::{}", q, func),
                    None => func.clone(),
                };
                let mut mir_args = Vec::new();
                for a in args {
                    mir_args.push(self.build_call_arg(a, shared)?);
                }
                Ok(MirRvalue::Call {
                    func: full_name,
                    args: mir_args,
                    is_intrinsic,
                    intrinsic_name,
                    generic_args: generic_args.clone(),
                })
            }
            HirExprKind::EnumVariantConstruction { enum_name, generic_args, variant_name, args } => {
                let mut mir_args = Vec::new();
                for a in args {
                    mir_args.push(self.build_call_arg(a, shared)?);
                }
                // 关键修复：这里原来用 `..` 把 HIR 节点自带的 generic_args
                // 直接丢掉了——sema 已经推导出 Some(x)/Ok(x) 这类构造具体
                // 实例化成了哪个类型（比如 Option<i32> 的 i32），这份信息
                // 传不到 MIR 层，monomorphic.rs 的泛型枚举单态化就没法做。
                Ok(MirRvalue::EnumVariantConstruction {
                    enum_name: enum_name.clone(),
                    generic_args: generic_args.clone(),
                    variant_name: variant_name.clone(),
                    args: mir_args,
                })
            }
            HirExprKind::EnumVariantAccess { enum_name, variant_name } => {
                // 注：ast::ExprKind::EnumVariantAccess 本身就没有
                // generic_args 字段（裸的 `EnumName::Variant` 访问，不像
                // EnumVariantConstruction 那样在语法层面就带类型实参），
                // 这里传空 Vec 不是漏传，是这条路径目前确实拿不到——如果
                // 以后要支持 `Option::<i32>::None` 这种写法，需要先在
                // ast.rs/hir.rs 里给 EnumVariantAccess 也加上 generic_args。
                Ok(MirRvalue::EnumVariantConstruction {
                    enum_name: enum_name.clone(),
                    generic_args: Vec::new(),
                    variant_name: variant_name.clone(),
                    args: Vec::new(),
                })
            }
            HirExprKind::StructInit { struct_name, generic_args, fields } => {
                let mut mir_fields = Vec::new();
                for (name, e) in fields {
                    mir_fields.push((name.clone(), self.build_expr(e, shared)?));
                }
                // 关键修复：同上，原来 `..` 把 generic_args 丢了。
                Ok(MirRvalue::StructInit {
                    struct_name: struct_name.clone(),
                    generic_args: generic_args.clone(),
                    fields: mir_fields,
                })
            }
            HirExprKind::ArrayLiteral(elements) => {
                let mut mir_elements = Vec::new();
                for e in elements {
                    mir_elements.push(self.build_expr(e, shared)?);
                }
                Ok(MirRvalue::ArrayLiteral(mir_elements))
            }
            HirExprKind::LackSlice(_ty) => Ok(MirRvalue::ArrayLiteral(Vec::new())),
            HirExprKind::Block(b) => {
                let last = self.build_block(b, shared)?;
                Ok(MirRvalue::Use(last.unwrap_or(MirOperand::Constant(crate::ast::Literal::Unit))))
            }
            HirExprKind::UnsafeBlock { body, .. } => {
                self.unsafe_depth += 1;
                self.push_stmt(MirStmt::EffectCheck { effect: "unsafe".to_string() });
                let last = self.build_block(body, shared)?;
                self.unsafe_depth -= 1;
                Ok(MirRvalue::Use(last.unwrap_or(MirOperand::Constant(crate::ast::Literal::Unit))))
            }
            HirExprKind::If { kind: _, cond, then_expr, else_expr } => {
                let cond_operand = self.build_expr(cond, shared)?;
                let then_block = self.new_block();
                let else_block = self.new_block();
                let end_block = self.new_block();
                self.set_terminator(MirTerminator::If { cond: cond_operand, then_block, else_block });

                // 目标变量的 base_id（不分配版本）
                let dest_base = self.new_temp(expr.ty.clone());
                let mut then_info = None;
                let mut else_info = None;

                // ---------- Then 分支 ----------
                self.switch_to_block(then_block);
                let then_diverges = matches!(then_expr.ty, Type::Never);
                let then_operand = self.build_expr(then_expr, shared)?;
                if then_diverges {
                    self.set_terminator(MirTerminator::Unreachable);
                } else {
                    let then_ver = self.new_version(dest_base);
                    let then_ssa = SsaLocal { base_id: dest_base, version: then_ver };
                    self.push_stmt(MirStmt::Assign {
                        dest: MirPlace::Ssa(then_ssa),
                        value: MirRvalue::Use(then_operand),
                    });
                    then_info = Some((then_block, then_ssa));
                    if self.current_terminator_is_placeholder() {
                        self.set_terminator(MirTerminator::Goto(end_block));
                    }
                }

                // ---------- Else 分支 ----------
                self.switch_to_block(else_block);
                match else_expr {
                    Some(e) => {
                        let else_diverges = matches!(e.ty, Type::Never);
                        let else_operand = self.build_expr(e, shared)?;
                        if else_diverges {
                            self.set_terminator(MirTerminator::Unreachable);
                        } else {
                            let else_ver = self.new_version(dest_base);
                            let else_ssa = SsaLocal { base_id: dest_base, version: else_ver };
                            self.push_stmt(MirStmt::Assign {
                                dest: MirPlace::Ssa(else_ssa),
                                value: MirRvalue::Use(else_operand),
                            });
                            else_info = Some((else_block, else_ssa));
                            if self.current_terminator_is_placeholder() {
                                self.set_terminator(MirTerminator::Goto(end_block));
                            }
                        }
                    }
                    None => {
                        // 没有 else 分支：sema 保证 then 分支是 Unit，给 dest_base 赋 Unit
                        let else_ver = self.new_version(dest_base);
                        let else_ssa = SsaLocal { base_id: dest_base, version: else_ver };
                        self.push_stmt(MirStmt::Assign {
                            dest: MirPlace::Ssa(else_ssa),
                            value: MirRvalue::Use(MirOperand::Constant(crate::ast::Literal::Unit)),
                        });
                        else_info = Some((else_block, else_ssa));
                        if self.current_terminator_is_placeholder() {
                            self.set_terminator(MirTerminator::Goto(end_block));
                        }
                    }
                }

                // ---------- End 块：插入 Phi ----------
                self.switch_to_block(end_block);
                let mut phi_values = Vec::new();
                if let Some((block, ssa)) = then_info {
                    phi_values.push((block, MirOperand::Move(MirPlace::Ssa(ssa))));
                }
                if let Some((block, ssa)) = else_info {
                    phi_values.push((block, MirOperand::Move(MirPlace::Ssa(ssa))));
                }

                // 至少有一个分支产生值（否则整个表达式发散，执行不到这里）
                if !phi_values.is_empty() {
                    let phi_ver = self.new_version(dest_base);
                    let phi_ssa = SsaLocal { base_id: dest_base, version: phi_ver };
                    self.push_stmt(MirStmt::Assign {
                        dest: MirPlace::Ssa(phi_ssa),
                        value: MirRvalue::Phi { values: phi_values },
                    });
                    Ok(MirRvalue::Use(MirOperand::Move(MirPlace::Ssa(phi_ssa))))
                } else {
                    // 两个分支都发散，执行不到这里，但类型系统要求返回，可以 unreachable!()
                    unreachable!()
                }
            }
            // ===== 明确留白，不是漏改 =====
            // Closure 需要处理捕获变量列表和生成匿名结构体，codegen 那边
            // 目前也没有闭包的生成策略；Range 作为独立表达式值（不是被
            // for 直接消费）目前语言里没有真实场景会用到。都是需要独立
            // 设计的功能，硬凑一个实现大概率是错的，不如显式报错等真正
            // 设计。（Match 已经在下面实现了，不再属于这一类留白。）
            HirExprKind::Match { cond, arms } => {
                // cond 只求值一次，落进一个临时变量——不管接下来走哪条
                // 路径，分支/解构里都可能要再读一次这个值（枚举取
                // payload、结构体取字段、数组取下标……），不能在这里就
                // 把它 Move 掉。
                let cond_op = self.build_expr(cond, shared)?;
                let cond_temp = self.new_temp(cond.ty.clone());
                // 关键修复：原来 `self.new_version(cond_temp)` 是直接内嵌
                // 在 push_stmt 参数的结构体字面量里算的——push_stmt 的
                // receiver `self` 先要了一个 &mut self（可变借用 A），
                // 参数里的 `self.new_version(...)` 又要一次 &mut self
                // （可变借用 B），两个可变借用同时活着，是真正的"两次可变
                // 借用重叠"，两阶段借用（two-phase borrow）救不了这种
                // 情况——它只能让"外层 &mut 借用 + 参数里的 &self 借用"
                // 共存（比如下面几处 `self.current_ssa(...)` 那种只读
                // 调用能直接嵌在 push_stmt 参数里，就是靠这个机制），
                // 两个都要 &mut 的调用没法这样叠。
                //
                // 拆成两步：先单独调用 self.new_version 拿到版本号存进
                // 局部变量（这次 &mut 借用调用完就还回去了），再用这个
                // 局部变量构造 SsaLocal 传给 push_stmt，这时候 push_stmt
                // 的 &mut 借用不会跟任何还没算完的借用重叠。
                let cond_ssa = SsaLocal { base_id: cond_temp, version: self.new_version(cond_temp) };
                self.push_stmt(MirStmt::Assign {
                    dest: MirPlace::Ssa(cond_ssa),
                    value: MirRvalue::Use(cond_op),
                });

                // ===== match 该怎么判断走哪条分支，完全取决于 cond 的
                // 类型，三条路径互不相通：
                //   1) 枚举——和类型，天然适合 Switch（原有逻辑，原样
                //      保留在下面）。
                //   2) 整数/布尔/字符——本身已经是标量，字面量模式就是
                //      "跟一个具体常量比较"，同样适合 Switch，只是判
                //      别式不是从聚合值里拆出来的，是把标量本身 Cast
                //      成 i64（复用已有的 MirRvalue::Cast，不新造机制；
                //      bool/char 落地成 Rust 的 `as i64` 天然可行，false/
                //      true 变成 0/1，char 变成码点）。
                //   3) 结构体/元组/数组——都不是和类型，没有"判别式"这
                //      回事，一个 match 表达式对着它们中的一种来匹配，
                //      语义上只可能是"无条件解构绑定"，不存在"没匹配
                //      上、换下一条分支再试"这种可能性——所以完全不碰
                //      Switch，也不需要为了分支而新开 block，当前块直
                //      接往下走，解构出来的绑定就是普通语句。因为没有
                //      "重试下一条"这回事，这三条路径都要求 arms 里有
                //      且只有一条对应的解构 arm（外加可选的 Wildcard 表
                //      示"解构但不关心任何字段/位置"）——多于一条在语义
                //      上是死代码，sema 应该已经保证了这一点，这里只是
                //      不假设、显式检查一遍。
                match &cond.ty {
                    // ---------- 结构体：无条件解构 ----------
                    Type::Struct(struct_name) => {
                        if arms.len() != 1 {
                            return Err(format!(
                                "match 条件是结构体类型 `{}`，结构体不是和类型，不支持多路\
                                 分支——只能有唯一一条解构 arm，实际有 {} 条（这本该在 sema \
                                 的可达性检查里被拦下）",
                                struct_name, arms.len()
                            ));
                        }
                        let arm = &arms[0];
                        self.push_scope();
                        match &arm.pattern {
                            Pattern::Struct { fields, .. } => {
                                for (field_name, binding_name) in fields {
                                    let field_ty = shared
                                        .struct_fields
                                        .get(struct_name)
                                        .and_then(|fs| fs.get(field_name))
                                        .cloned()
                                        .ok_or_else(|| format!(
                                            "struct `{}` has no field `{}`", struct_name, field_name
                                        ))?;
                                    let binding_id = self.new_local(Some(binding_name.clone()), field_ty, false, true);
                                    let binding_ver = self.new_version(binding_id);
                                    self.push_stmt(MirStmt::Assign {
                                        dest: MirPlace::Ssa(SsaLocal { base_id: binding_id, version: binding_ver }),
                                        value: MirRvalue::Use(MirOperand::Move(MirPlace::Field {
                                            base: Box::new(MirPlace::Ssa(self.current_ssa(cond_temp))),
                                            field: field_name.clone(),
                                        })),
                                    });
                                    self.bind(binding_name.clone(), binding_id);
                                    // 关键修复（找回上一轮的修复）：这一
                                    // 步把 cond_temp 的某个字段 Move 走
                                    // 了，是对 cond_temp 的一次部分移动，
                                    // cond_temp 自己也登记在当前作用域
                                    // 里，之后 pop_scope 给它补 Drop 的
                                    // 时候要知道这件事，不然会对一个已经
                                    // 被部分移动过的值调用 drop()。
                                    self.moved.insert(self.current_ssa(cond_temp));
                                }
                            }
                            Pattern::Wildcard => {}
                            other => {
                                return Err(format!(
                                    "match 条件是结构体类型 `{}`，但这个 arm 的模式是 {:?}",
                                    struct_name, other
                                ));
                            }
                        }
                        let operand = self.build_expr(&arm.expr, shared)?;
                        self.pop_scope();
                        Ok(MirRvalue::Use(operand))
                    }

                    // 对应 ast.rs 里新增的 Type::Tuple(Vec<Type>)。
                    Type::Tuple(elem_tys) => {
                        if arms.len() != 1 {
                            return Err(format!(
                                "match 条件是元组类型，元组不是和类型，不支持多路分支——只能\
                                 有唯一一条解构 arm，实际有 {} 条", arms.len()
                            ));
                        }
                        let arm = &arms[0];
                        self.push_scope();
                        match &arm.pattern {
                            Pattern::Tuple(bindings) => {
                                for (i, binding_name) in bindings.iter().enumerate() {
                                    // 这个位置写 None（对应源码里的 `_`）
                                    // 表示不绑定，跳过——跟 Wildcard 整条
                                    // 不绑定是同一个约定，只是落到单个
                                    // 位置上。
                                    let binding_name = match binding_name {
                                        Some(name) => name,
                                        None => continue,
                                    };
                                    let elem_ty = elem_tys.get(i).cloned().ok_or_else(|| {
                                        "tuple pattern has more bindings than the tuple type has elements".to_string()
                                    })?;
                                    let binding_id = self.new_local(Some(binding_name.clone()), elem_ty, false, true);
                                    let binding_ver = self.new_version(binding_id);
                                    self.push_stmt(MirStmt::Assign {
                                        dest: MirPlace::Ssa(SsaLocal { base_id: binding_id, version: binding_ver }),
                                        // 元组的第 i 个位置复用
                                        // MirPlace::Field，用十进制下标
                                        // 字符串当"字段名"——不新增一个
                                        // MirPlace 变体：跟 EnumPayload/
                                        // Index 注释里反复强调的原则一
                                        // 样，能用已有的落点就不另起一
                                        // 个，codegen 落地成 Rust 元组
                                        // 下标 `.0`/`.1` 时，字段名恰好
                                        // 就是要拼的那个数字。
                                        value: MirRvalue::Use(MirOperand::Move(MirPlace::Field {
                                            base: Box::new(MirPlace::Ssa(self.current_ssa(cond_temp))),
                                            field: i.to_string(),
                                        })),
                                    });
                                    self.bind(binding_name.clone(), binding_id);
                                    // 关键修复（找回上一轮的修复）：同结
                                    // 构体解构那处的说明，这也是对
                                    // cond_temp 的一次部分移动。
                                    self.moved.insert(self.current_ssa(cond_temp));
                                }
                            }
                            Pattern::Wildcard => {}
                            other => {
                                return Err(format!(
                                    "match 条件是元组类型，但这个 arm 的模式是 {:?}", other
                                ));
                            }
                        }
                        let operand = self.build_expr(&arm.expr, shared)?;
                        self.pop_scope();
                        Ok(MirRvalue::Use(operand))
                    }
                    Type::Array(elem_ty, _len) => {
                        if arms.len() != 1 {
                            return Err(format!(
                                "match 条件是数组类型，数组不是和类型，不支持多路分支——只能\
                                 有唯一一条解构 arm，实际有 {} 条", arms.len()
                            ));
                        }
                        let arm = &arms[0];
                        self.push_scope();
                        match &arm.pattern {
                            Pattern::Array(bindings) => {
                                for (i, binding_name) in bindings.iter().enumerate() {
                                    let binding_name = match binding_name {
                                        Some(name) => name,
                                        None => continue,
                                    };
                                    let binding_id = self.new_local(Some(binding_name.clone()), (**elem_ty).clone(), false, true);
                                    let binding_ver = self.new_version(binding_id);
                                    self.push_stmt(MirStmt::Assign {
                                        dest: MirPlace::Ssa(SsaLocal { base_id: binding_id, version: binding_ver }),
                                        value: MirRvalue::Use(MirOperand::Move(MirPlace::Index {
                                            base: Box::new(MirPlace::Ssa(self.current_ssa(cond_temp))),
                                            // 关键修复：`Literal::Int` 这
                                            // 个变体在 ast.rs 这一轮改动
                                            // 里已经不存在了（Literal 现在
                                            // 按位宽/符号拆成了
                                            // Int8..Int128/UInt8..UInt128/
                                            // Isize/Usize 一堆变体），原来
                                            // 这行代码引用的是一个已经被
                                            // 删掉的枚举变体，编译不过。
                                            // 数组下标在 Rust 里必须是
                                            // usize（`arr[idx]` 要求
                                            // `idx: usize`），`i` 本来就是
                                            // `enumerate()` 给出的 usize，
                                            // 直接用 Literal::Usize 存，
                                            // 不用再转 i64 又转回来。
                                            index: Box::new(MirOperand::Constant(crate::ast::Literal::Usize(i))),
                                        })),
                                    });
                                    self.bind(binding_name.clone(), binding_id);
                                    // 关键修复（找回上一轮的修复）：同上，
                                    // 数组下标提取也是对 cond_temp 的一次
                                    // 部分移动。
                                    self.moved.insert(self.current_ssa(cond_temp));
                                }
                            }
                            Pattern::Wildcard => {}
                            other => {
                                return Err(format!(
                                    "match 条件是数组类型，但这个 arm 的模式是 {:?}", other
                                ));
                            }
                        }
                        let operand = self.build_expr(&arm.expr, shared)?;
                        self.pop_scope();
                        Ok(MirRvalue::Use(operand))
                    }

                    // ---------- 枚举：原有逻辑，原样保留 ----------
                    Type::Enum(enum_name) => {
                        let cond_enum_name = enum_name.clone();

                        // 1. 计算判别式，存入 disc_temp（用 SSA 版本）
                        let disc_temp = self.new_temp(Type::I64);
                        let disc_ver = self.new_version(disc_temp);
                        let disc_ssa = SsaLocal { base_id: disc_temp, version: disc_ver };
                        self.push_stmt(MirStmt::Assign {
                            dest: MirPlace::Ssa(disc_ssa),
                            value: MirRvalue::Discriminant {
                                value: MirOperand::Copy(MirPlace::Ssa(self.current_ssa(cond_temp))),
                                enum_name: cond_enum_name,
                            },
                        });

                        let end_block = self.new_block();
                        let dest_base = self.new_temp(expr.ty.clone());
                        let mut arm_infos: Vec<(usize, &HirExpr, Option<(String, String)>)> = Vec::new();

                        let mut targets = Vec::new();
                        let mut default_block = None;

                        // 2. 收集所有分支
                        for arm in arms {
                            match &arm.pattern {
                                Pattern::EnumVariant { enum_name, variant_name } => {
                                    let idx = *shared
                                        .variant_indices
                                        .get(&(enum_name.clone(), variant_name.clone()))
                                        .ok_or_else(|| format!("unknown enum variant: {}::{}", enum_name, variant_name))? as i64;
                                    let block = self.new_block();
                                    // 关键修复：`Literal::Int` 不存在了
                                    // （见上面数组下标那处的说明），
                                    // disc_temp 声明的是 Type::I64，这里
                                    // 要用跟它匹配的 Int64。
                                    targets.push((Literal::Int64(idx), block));
                                    arm_infos.push((block, &arm.expr, None));
                                }
                                Pattern::EnumVariantWithBinding { enum_name, variant_name, binding } => {
                                    let idx = *shared
                                        .variant_indices
                                        .get(&(enum_name.clone(), variant_name.clone()))
                                        .ok_or_else(|| format!("unknown enum variant: {}::{}", enum_name, variant_name))? as i64;
                                    let block = self.new_block();
                                    targets.push((Literal::Int64(idx), block));
                                    arm_infos.push((block, &arm.expr, Some((binding.clone(), variant_name.clone()))));
                                }
                                Pattern::Wildcard => {
                                    let block = self.new_block();
                                    default_block = Some(block);
                                    arm_infos.push((block, &arm.expr, None));
                                }
                                other => {
                                    return Err(format!(
                                        "match 条件是枚举类型，但这个 arm 的模式是 {:?}——只能用 \
                                         EnumVariant/EnumVariantWithBinding/Wildcard 匹配",
                                        other
                                    ));
                                }
                            }
                        }

                        let default = default_block.unwrap_or_else(|| self.new_block());

                        // 3. 设置 Switch，discr 使用 disc_ssa
                        self.set_terminator(MirTerminator::Switch {
                            discr: MirOperand::Move(MirPlace::Ssa(disc_ssa)),
                            discr_ty: Type::I64,
                            targets,
                            default,
                        });

                        // 4. 处理每个分支，记录每个分支产生的 SSA 版本
                        let mut branch_results = Vec::new(); // (block_id, ssa_local)

                        for (block, arm_expr, binding_info) in arm_infos {
                            self.switch_to_block(block);
                            self.push_scope();

                            // 处理 binding（如果有）
                            if let Some((binding_name, variant_name)) = &binding_info {
                                let enum_name = shared
                                    .variant_to_enum
                                    .get(variant_name)
                                    .cloned()
                                    .ok_or_else(|| format!("unknown enum variant: {}", variant_name))?;
                                let payload_ty = shared
                                    .variant_payload_types
                                    .get(&(enum_name.clone(), variant_name.clone()))
                                    .cloned()
                                    .ok_or_else(|| format!(
                                        "variant `{}` has no payload but pattern binds `{}`",
                                        variant_name, binding_name
                                    ))?;
                                let binding_id = self.new_local(Some(binding_name.clone()), payload_ty, false, true);
                                let binding_ver = self.new_version(binding_id);
                                self.push_stmt(MirStmt::Assign {
                                    dest: MirPlace::Ssa(SsaLocal { base_id: binding_id, version: binding_ver }),
                                    value: MirRvalue::Use(MirOperand::Move(MirPlace::EnumPayload {
                                        base: Box::new(MirPlace::Ssa(self.current_ssa(cond_temp))),
                                        enum_name,
                                        variant_name: variant_name.clone(),
                                    })),
                                });
                                self.bind(binding_name.clone(), binding_id);
                                // 关键修复（找回上一轮的修复）：取
                                // payload 是对 cond_temp 的一次部分移动，
                                // 跟结构体/元组/数组解构那三处是同一个
                                //道理。只在"确实有 binding"这个分支里
                                // 才标记——纯 EnumVariant（不带 payload
                                // 绑定）和 Wildcard 分支不会碰 cond_temp
                                // 的任何部分，那些路径下 cond_temp 后面
                                // 正常 Drop 就行。
                                self.moved.insert(self.current_ssa(cond_temp));
                            }

                            let arm_diverges = matches!(arm_expr.ty, Type::Never);
                            let operand = self.build_expr(arm_expr, shared)?;
                            if arm_diverges {
                                self.set_terminator(MirTerminator::Unreachable);
                            } else {
                                let arm_ver = self.new_version(dest_base);
                                let arm_ssa = SsaLocal { base_id: dest_base, version: arm_ver };
                                self.push_stmt(MirStmt::Assign {
                                    dest: MirPlace::Ssa(arm_ssa),
                                    value: MirRvalue::Use(operand),
                                });
                                branch_results.push((block, arm_ssa));
                                if self.current_terminator_is_placeholder() {
                                    self.set_terminator(MirTerminator::Goto(end_block));
                                }
                            }
                            self.pop_scope();
                        }

                        // 5. 切换到 end_block，插入 Phi
                        self.switch_to_block(end_block);
                        if branch_results.is_empty() {
                            // 所有分支都发散，执行不到这里
                            unreachable!()
                        } else {
                            let phi_values: Vec<(usize, MirOperand)> = branch_results
                                .into_iter()
                                .map(|(block, ssa)| (block, MirOperand::Move(MirPlace::Ssa(ssa))))
                                .collect();

                            let phi_ver = self.new_version(dest_base);
                            let phi_ssa = SsaLocal { base_id: dest_base, version: phi_ver };
                            self.push_stmt(MirStmt::Assign {
                                dest: MirPlace::Ssa(phi_ssa),
                                value: MirRvalue::Phi { values: phi_values },
                            });
                            Ok(MirRvalue::Use(MirOperand::Move(MirPlace::Ssa(phi_ssa))))
                        }
                    }

                    // ---------- 整数/布尔/字符：标量字面量，复用 Switch ----------
                    // 新增：这三种类型本身已经是标量，字面量模式就是
                    // "判别式取某个具体的 i64 值就跳到哪个块"，跟枚举
                    // match 走的是同一套 Switch 骨架，唯一的区别是"discr
                    // 怎么算出来"——枚举要先从聚合值里拆判别式
                    // （MirRvalue::Discriminant），标量类型本身已经是
                    // 标量了，不需要拆，只需要统一 Cast 成 i64（复用
                    // 已有的 MirRvalue::Cast，不新造机制）：bool 的 Cast
                    // 落地成 Rust 的 `as i64`（false/true 变成 0/1），
                    // char 落地成 `as i64`（拿到码点），整数自己 Cast
                    // 成 i64 在源类型已经是 i64 时是恒等操作——不管原来
                    // 是 i32/i64/u8/bool/char 里的哪个，都走同一条 Cast
                    // 语句，不用在这里分开重复代码。
                    _ => {
                        // 1. 直接使用 cond_temp 作为判别式，不 Cast
                        let discr_ty = cond.ty.clone();
                        let discr = MirOperand::Copy(MirPlace::Ssa(self.current_ssa(cond_temp)));

                        let end_block = self.new_block();
                        let dest_base = self.new_temp(expr.ty.clone());

                        let mut targets = Vec::new();
                        let mut default_block = None;
                        // 关键修复：这个 Vec 原来也叫 branch_results，
                        // 跟第 4 步"记录每个分支产生的 SSA 版本"那个
                        // Vec 撞了同一个名字——第 4 步那句
                        // `let mut branch_results = Vec::new();` 会把
                        // 这里收集到的 (block, &arm.expr) 列表直接遮蔽
                        // 掉，然后下面的 for 循环写的是
                        // `for (block, arm_expr) in arm_results`——
                        // `arm_results` 这个名字在整个函数里根本没有
                        // 声明过，编译不过（E0425）。改名成 arm_infos，
                        // 跟上面枚举 match 分支里同样用途的变量命名
                        // 保持一致，两个 Vec 各自独立，不会互相覆盖。
                        let mut arm_infos: Vec<(usize, &HirExpr)> = Vec::new();

                        // 2. 收集所有分支（targets 存 Literal）
                        for arm in arms {
                            match &arm.pattern {
                                Pattern::IntLiteral(v) => {
                                    let block = self.new_block();
                                    // 关键修复：`Literal::Int` 不存在了，
                                    // 按 discr 的具体类型（i8/u32/...）
                                    // 转换成对应的 Literal 变体——见
                                    // int_literal_for_type 的说明。
                                    let lit = Self::int_literal_for_type(*v, &discr_ty)?;
                                    targets.push((lit, block));
                                    arm_infos.push((block, &arm.expr));
                                }
                                Pattern::BoolLiteral(v) => {
                                    let block = self.new_block();
                                    targets.push((Literal::Bool(*v), block));
                                    arm_infos.push((block, &arm.expr));
                                }
                                Pattern::CharLiteral(c) => {
                                    let block = self.new_block();
                                    targets.push((Literal::Char(*c), block));
                                    arm_infos.push((block, &arm.expr));
                                }
                                Pattern::Wildcard => {
                                    let block = self.new_block();
                                    default_block = Some(block);
                                    arm_infos.push((block, &arm.expr));
                                }
                                other => {
                                    return Err(format!(
                                        "match 条件是标量类型（整数/布尔/字符），但这个 arm 的\
                                         模式是 {:?}——只能用 \
                                         IntLiteral/BoolLiteral/CharLiteral/Wildcard 匹配",
                                        other
                                    ));
                                }
                            }
                        }

                        // 关键修复：跟枚举 match 不同，字面量分支不可能
                        // 自证穷尽（比如 i32 的取值范围远不是几个字面量
                        // 分支能覆盖完的），缺 Wildcard 分支时不能悄悄
                        // 放过——之前这里的错误信息被偷懒地写成裸
                        // `"..."`，改成一句真正能定位问题的话。
                        let default = default_block.ok_or_else(|| {
                            "match 条件是标量类型，但没有 Wildcard 兜底分支——标量字面量\
                             模式不可能自证穷尽，这应该在 sema 阶段就被拦下".to_string()
                        })?;

                        // 3. 设置 Switch
                        self.set_terminator(MirTerminator::Switch {
                            discr,
                            discr_ty,
                            targets,
                            default,
                        });

                        // 4. 处理每个分支，记录 SSA 版本
                        let mut branch_results = Vec::new();

                        for (block, arm_expr) in arm_infos {
                            self.switch_to_block(block);
                            let arm_diverges = matches!(arm_expr.ty, Type::Never);
                            let operand = self.build_expr(arm_expr, shared)?;
                            if arm_diverges {
                                self.set_terminator(MirTerminator::Unreachable);
                            } else {
                                let arm_ver = self.new_version(dest_base);
                                let arm_ssa = SsaLocal { base_id: dest_base, version: arm_ver };
                                self.push_stmt(MirStmt::Assign {
                                    dest: MirPlace::Ssa(arm_ssa),
                                    value: MirRvalue::Use(operand),
                                });
                                branch_results.push((block, arm_ssa));
                                if self.current_terminator_is_placeholder() {
                                    self.set_terminator(MirTerminator::Goto(end_block));
                                }
                            }
                        }

                        // 5. End block：Phi
                        self.switch_to_block(end_block);
                        if branch_results.is_empty() {
                            unreachable!()
                        } else {
                            let phi_values: Vec<(usize, MirOperand)> = branch_results
                                .into_iter()
                                .map(|(block, ssa)| (block, MirOperand::Move(MirPlace::Ssa(ssa))))
                                .collect();

                            let phi_ver = self.new_version(dest_base);
                            let phi_ssa = SsaLocal { base_id: dest_base, version: phi_ver };
                            self.push_stmt(MirStmt::Assign {
                                dest: MirPlace::Ssa(phi_ssa),
                                value: MirRvalue::Phi { values: phi_values },
                            });
                            Ok(MirRvalue::Use(MirOperand::Move(MirPlace::Ssa(phi_ssa))))
                        }
                    }
                }
            }

            HirExprKind::Closure { .. } => {
                Err("MIR lowering for Closure: 捕获变量与闭包结构体生成策略还没设计".to_string())
            }
            
            HirExprKind::Range { .. } => {
                Err("MIR lowering for bare Range: Range 目前只应该在 for 循环展开后被消费，独立出现说明 elaborate.rs 该做的展开还没做".to_string())
            }
            // ===== Never 支持接到了"语句位置"、"if 分支位置"、现在加上
            // "match 分支位置"这三个落点（HirStmt::Expr / build_fn 收尾 /
            // If 分支 / 这里的 Match 分支），但还没有往下延伸到任意子
            // 表达式里，比如 `let x = 1 + panic();` 这种把 never 表达式
            // 嵌在算术/调用参数等更深层位置的写法，目前依然会被当成
            // 普通表达式求值，不会提前把块标成 Unreachable。要做对需要
            // 让每个 build_expr 调用点都能"劈开"当前块（跟 If/Match/
            // Return 现在做的事一样），牵动面比这几轮都大，先不做。
        }
    }

    fn build_call_arg(&mut self, arg: &HirCallArg, shared: &SharedContext) -> Result<MirOperand, String> {
        match arg {
            HirCallArg::Positional(e) => self.build_expr(e, shared),
            HirCallArg::Named(_, e) => self.build_expr(e, shared),
        }
    }
}