// state.rs
//
// 从 mir_builder.rs 拆出来的一部分：MirBuilder 自己的构建期状态管理——
// 局部变量表怎么增删、SSA 版本号怎么发、作用域怎么进出、基本块/语句/
// 终结指令怎么写。这些函数不关心"正在构建的是什么表达式"，只关心
// "MirBuilder 手上这几张表该怎么维护"，跟 build_expr/build_stmt 那种
// "HIR 节点 -> MIR 节点"的转换逻辑是不同层次的关注点，分开之后
// mir_builder.rs 里剩下的才是真正的"翻译"代码。

use crate::ast::Type;
// 关键修复：`crate::hir::Literal` 是私有的枚举导入（E0603）——hir.rs
// 内部大概率是用不带 pub 的 `use crate::ast::Literal;` 引进来自用的，
// 只在 hir.rs 自己模块内部可见，不构成 hir 对外公开的路径，state.rs
// 作为平级模块按名字导入会被挡。回头看，mir_builder.rs 原来的代码能
// 用裸 `Literal::Int64(...)` 这类写法，靠的其实是 `use crate::mir::*;`
// ——mir.rs 是 `pub use crate::ast::{Type, ..., Literal};`，真正公开
// 转发了这个类型，不是 hir.rs 那边的功劳。这里干脆直接从定义它的
// ast.rs 导入，不依赖某个中间模块顺手公开转发了它这件事。
use crate::ast::Literal;
use crate::mir::*;
use crate::mir_builder::MirBuilder;
use std::collections::HashMap;

impl MirBuilder {
    // -------- 局部变量 --------
    pub(crate) fn new_local(&mut self, name: Option<String>, ty: Type, mutable: bool, add_to_scope: bool) -> usize {
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
    pub(crate) fn new_param_local(&mut self, name: String, ty: Type) -> usize {
        let id = self.locals.len();
        self.locals.push(MirLocal { id, name: Some(name), ty, mutable: false, persist: false, is_param: true });
        id
    }

    pub(crate) fn new_persist_local(&mut self, name: Option<String>, ty: Type, mutable: bool) -> usize {
        let id = self.locals.len();
        self.locals.push(MirLocal { id, name, ty, mutable, persist: true, is_param: false });
        self.scope_vars.last_mut().unwrap().push(id);
        id
    }

    pub(crate) fn new_version(&mut self, base_id: usize) -> u32 {
        let ver = self.ssa_versions.get(&base_id).copied().unwrap_or(0) + 1;
        self.ssa_versions.insert(base_id, ver);
        ver
    }

    pub(crate) fn new_temp(&mut self, ty: Type) -> usize {
        self.new_local(None, ty, false, true)
    }

    pub(crate) fn current_ssa(&self, base_id: usize) -> SsaLocal {
        let version = self.ssa_versions.get(&base_id).copied().unwrap_or(0);
        SsaLocal { base_id, version }
    }

    // 关键新增：Pattern::IntLiteral 只存了一个裸 i64（这是 ast.rs 里
    // Pattern 自己的限制，还没跟着这一轮 Literal 拆分成按位宽/符号
    // 区分的一堆变体一起升级——也就是说目前没法用字面量模式匹配超出
    // i64 范围的 i128/u128 值，这是个已知的、比这次修复范围更大的
    // 缺口，这里先不动 ast.rs，只保证"i64 范围内的值，按 discr 的具体
    // 类型转换成正确的 Literal 变体"这件事是对的）。
    pub(crate) fn int_literal_for_type(v: i64, ty: &Type) -> Result<Literal, String> {
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
    pub(crate) fn push_scope(&mut self) {
        self.scope.push(HashMap::new());
        self.scope_vars.push(Vec::new());
    }

    pub(crate) fn pop_scope(&mut self) {
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
                // MirBuilder.moved 字段的说明，以及往里登记的地方。
                if !self.moved.contains(&ssa) {
                    self.push_stmt(MirStmt::Drop { place: MirPlace::Ssa(ssa) });
                }
            }
        }
        self.scope.pop();
    }

    pub(crate) fn bind(&mut self, name: String, id: usize) {
        self.scope.last_mut().unwrap().insert(name, id);
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<usize> {
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
    pub(crate) fn new_block(&mut self) -> usize {
        let id = self.blocks.len();
        self.blocks.push(MirBlock {
            id,
            stmts: Vec::new(),
            terminator: MirTerminator::Unreachable,
        });
        id
    }

    pub(crate) fn switch_to_block(&mut self, id: usize) {
        self.current_block = id;
    }

    pub(crate) fn push_stmt(&mut self, stmt: MirStmt) {
        self.blocks[self.current_block].stmts.push(stmt);
    }

    pub(crate) fn set_terminator(&mut self, term: MirTerminator) {
        self.blocks[self.current_block].terminator = term;
    }

    pub(crate) fn current_terminator_is_placeholder(&self) -> bool {
        match self.blocks[self.current_block].terminator {
            MirTerminator::Unreachable => true,
            _ => false,
        }
    }
}
