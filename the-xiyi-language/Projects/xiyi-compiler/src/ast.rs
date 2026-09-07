// 程序 = 一组项
#[derive(Debug, PartialEq, Clone)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Item {
    FnDef(FnDef),
    StructDef(StructDef),
    EnumDef(EnumDef),
    ConstDef(ConstDef),
    ModelDef(ModelDef),
    ProtoDef(ProtoDef),
    Use(UseStmt),
    Implement(ImplementDef),
    Interface(InterfaceDef),
}

// ===== 属性系统 =====
#[derive(Debug, PartialEq, Clone)]
pub struct Attribute {
    pub name: String,
    pub args: Vec<AttributeArg>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum AttributeArg {
    Ident(String),
    StringLit(String),
    Int(i64),
    Float(f64),
    Rational(String),
    KeyValue(String, Box<AttributeArg>),
}

// ===== 泛型参数（已修改） =====
#[derive(Debug, PartialEq, Clone)]
pub enum GenericParam {
    Type { name: String, bounds: Vec<String> },
}

// ===== 函数定义 =====
#[derive(Debug, PartialEq, Clone)]
pub struct FnDef {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Block,
}

// ===== 结构体定义 =====
#[derive(Debug, PartialEq, Clone)]
pub struct StructDef {
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub fields: Vec<StructField>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct StructField {
    pub name: String,
    pub ty: Type,
}

// ===== 枚举定义 =====
#[derive(Debug, PartialEq, Clone)]
pub struct EnumDef {
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub ty: Option<Type>,
}

// ===== const 常量定义 =====
#[derive(Debug, PartialEq, Clone)]
pub struct ConstDef {
    pub name: String,
    pub ty: Type,
    pub value: Box<Expr>,
}

// ===== model 块定义 =====
#[derive(Debug, PartialEq, Clone)]
pub struct ModelDef {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub fields: Vec<ModelField>,
    pub functions: Vec<FnDef>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct ModelField {
    pub name: String,
    pub ty: Type,
}

// ===== proto 协议定义 =====
#[derive(Debug, PartialEq, Clone)]
pub struct ProtoDef {
    pub name: String,
    pub variants: Vec<ProtoVariant>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct ProtoVariant {
    pub name: String,
    pub ty: Option<Type>,
}

// ===== use 语句 =====
#[derive(Debug, PartialEq, Clone)]
pub struct UseStmt {
    pub path: String,
    pub alias: Option<String>,
}

// ===== implement 块 =====
#[derive(Debug, PartialEq, Clone)]
pub struct ImplementDef {
    pub attributes: Vec<Attribute>,
    pub generic_params: Vec<GenericParam>,
    pub target_type: Type,
    pub interface_name: Option<String>,
    pub functions: Vec<FnDef>,
    pub where_clause: Vec<WhereClause>,
}

// ===== interface 定义 =====
#[derive(Debug, PartialEq, Clone)]
pub struct InterfaceDef {
    pub attributes: Vec<Attribute>,
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub methods: Vec<FnSig>,
}

// ===== 方法签名（用于 interface） =====
#[derive(Debug, PartialEq, Clone)]
pub struct FnSig {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub generic_params: Vec<GenericParam>,
}

// ===== where 子句 =====
#[derive(Debug, PartialEq, Clone)]
pub struct WhereClause {
    pub type_name: String,
    pub bounds: Vec<String>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

// ===== 各类语句 =====
#[derive(Debug, PartialEq, Clone)]
pub struct ForStmt {
    pub var: String,
    pub iterable: Box<Expr>,
    pub body: Block,
}

#[derive(Debug, PartialEq, Clone)]
pub struct AssignStmt {
    // ===== 关键修改：name: String -> target: Box<Expr> =====
    // 原来只能表达"给一个裸变量名赋值"（i = expr），self.len = expr、
    // arr[i] = expr 这类写法完全表达不出来。不新开一套"左值"语法，直接
    // 复用现成的 Expr——Ident/FieldAccess/Index 本来就都是合法表达式，
    // "这个表达式能不能被赋值"这件事留给 sema 去检查，ast 层不区分。
    pub target: Box<Expr>,
    pub expr: Box<Expr>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct LoopStmt {
    pub body: Block,
}

#[derive(Debug, PartialEq, Clone)]
pub struct BreakStmt {}

#[derive(Debug, PartialEq, Clone)]
pub struct MatchExpr {
    pub cond: Box<Expr>,
    pub arms: Vec<MatchArm>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub expr: Box<Expr>,
}

// ===== Pattern 枚举 =====
#[derive(Debug, PartialEq, Clone)]
pub enum Pattern {
    EnumVariant {
        enum_name: String,
        variant_name: String,
    },
    EnumVariantWithBinding {
        enum_name: String,
        variant_name: String,
        binding: String,
    },
    Wildcard,
    // ===== 新增：标量字面量模式 =====
    // int/bool/char 三种字面量在语义上都只是"跟一个具体常量比较"，直接
    // 存各自的 Rust 原生类型即可，不用再包一层 Literal——模式匹配用不
    // 到 Literal 里 Float/String/Unit/ByteString 等其它变体，没必要为
    // 了复用 Literal 而放宽这里能接受的种类。
    IntLiteral(i64),
    BoolLiteral(bool),
    CharLiteral(char),
    // ===== 新增：结构体/元组/数组解构 =====
    // 这三种类型都不是和类型（sum type），不参与"判别式取哪个值"这种
    // 比较——一个 match 表达式对着它们中的一种类型来匹配，只可能是
    // "无条件解构绑定"，不是多路分支（mir_builder.rs 里这三种模式完
    // 全不走 Switch，且要求所在的 match 有且只有一条 arm）。
    //
    // 每个字段/位置只支持"绑定到一个新局部变量"这一层，不支持在字段/
    // 位置上继续嵌套子模式（比如 `Point { x: 0, y }` 或
    // `(a, Point { .. })`）——这跟 EnumVariantWithBinding.binding 是同
    // 一个"只绑一层、不递归"的扁平化设计，简单场景够用；真要支持嵌套
    // 模式是明显更大的一块工作，留给以后单独做。
    Struct {
        struct_name: String,
        // (字段名, 绑定成的新局部变量名)。不用列出结构体的全部字段——
        // 跟 Rust 的 `Point { x, .. }` 部分模式类似，只解构关心的那几个。
        fields: Vec<(String, String)>,
    },
    // 按位置绑定；某个位置写 None 表示对应源码里的 `_`（这个位置存在
    // 但不关心，不绑定成变量）。
    Tuple(Vec<Option<String>>),
    // 按下标绑定的定长数组解构；None 的含义跟 Tuple 一致。
    Array(Vec<Option<String>>),
}

// ===== 语句枚举 =====
#[derive(Debug, PartialEq, Clone)]
pub enum Stmt {
    Let(LetStmt),
    ExprStmt(Expr),
    Return(Option<Expr>),
    While(WhileStmt),
    For(ForStmt),
    Assign(AssignStmt),
    Loop(LoopStmt),
    Break(BreakStmt),
    UnsafeBlock(UnsafeBlockStmt),
}

#[derive(Debug, PartialEq, Clone)]
pub struct LetStmt {
    pub name: String,
    pub ty: Option<Type>,
    pub init: Box<Expr>,
    pub mutable: bool,
    pub persist: bool,
}

#[derive(Debug, PartialEq, Clone)]
pub struct UnsafeBlockStmt {
    pub kind: UnsafeKind,
    pub body: Block,
}

#[derive(Debug, PartialEq, Clone)]
pub enum UnsafeKind {
    Normal,
    Verify,
}

#[derive(Debug, PartialEq, Clone)]
pub struct WhileStmt {
    pub cond: Box<Expr>,
    pub body: Block,
}

// ===== 调用参数 =====
#[derive(Debug, PartialEq, Clone)]
pub enum CallArg {
    Positional(Expr),
    Named(String, Expr),
}

// ===== 表达式 =====
#[derive(Debug, PartialEq, Clone)]
pub struct Expr {
    pub id: usize,
    pub kind: ExprKind,
}

#[derive(Debug, PartialEq, Clone)]
pub enum ExprKind {
    Literal(Literal),
    Ident(String),
    Sym(String),
    BinaryOp {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        // ===== 新增：限定路径调用（如 Rational::gcd(a, b)）=====
        // None = 普通调用/方法调用（跟以前完全一样，不影响现有代码）
        // Some("Rational") = 静态限定调用，不是枚举变体构造、也不是方法调用
        qualifier: Option<String>,
        func: String,
        args: Vec<CallArg>,
        is_method: bool,
    },
    Block(Block),
    StructInit {
        struct_name: String,
        fields: Vec<(String, Expr)>,
    },
    FieldAccess {
        struct_expr: Box<Expr>,
        field_name: String,
    },
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
    },
    EnumVariantAccess {
        enum_name: String,
        variant_name: String,
    },
    EnumVariantConstruction {
        enum_name: String,
        variant_name: String,
        args: Vec<CallArg>,
    },
    Match(MatchExpr),
    Closure {
        param: String,
        body: Box<Expr>,
    },
    If {
        // ===== 新增：Normal / Lack，跟 UnsafeKind::{Normal,Verify} 同一个
        // 模式——Lack 表示 `lack if cond { ... }`，语义上强制没有 else、
        // then 分支必须是 Unit（纯副作用，不产出值）；Normal 就是原来的
        // if，必须带 else。两条规则的强制检查在 sema 层做，这里只是
        // 结构上把"写的是哪种 if"记下来。
        kind: IfKind,
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Option<Box<Expr>>,
    },
    ArrayLiteral(Vec<Expr>),
    UnsafeBlock(UnsafeBlockStmt),
    // ===== 新增：一元运算符（目前用于一元负号 -x，Not 一并加上，
    // 方便以后把 not(x) 那个坑改成真正的 !x 语法时直接复用这个节点）=====
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    // ===== 新增：as 类型转换，如 x as u128 =====
    Cast {
        expr: Box<Expr>,
        ty: Type,
    },
    // ===== 新增：索引表达式 expr[idx]，如 bytes[i] =====
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    // ===== 新增：lack &[T] 空切片字面量，类型是 &[T]，长度恒为 0 =====
    // T 必须是具体类型（禁止泛型参数/never/impl Trait，这条约束交给
    // sema 检查），这里直接存 Type，不需要额外包一层结构。
    LackSlice(Type),
}

// ===== 一元运算符 =====
// 关键修复：补上 Copy——simplify.rs 里 Calc::eval_unary_op(*op, ...) 这种
// 写法要对着 &UnaryOp 解引用取值，没有 Copy 编译不过（E0507）。纯枚举、
// 不带数据，加 Copy 完全安全，不影响任何已有用法（Copy 类型依然可以
// 正常 .clone()）。
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Not,
}

// ===== if 的两种语义标记，跟 UnsafeKind::{Normal,Verify} 同一个模式 =====
// Normal：普通 if，必须带 else，两分支类型必须一致
// Lack：`lack if cond { ... }`，显式声明"没有 else、纯副作用"，
//        强制没有 else 分支、then 分支必须是 Unit 类型
#[derive(Debug, PartialEq, Clone)]
pub enum IfKind {
    Normal,
    Lack,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Literal {
    // 有符号整数
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Int128(i128),
    // 无符号整数
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    UInt128(u128),
    // 平台相关
    Isize(isize),
    Usize(usize),
    // 浮点数
    Float16(f32),   // 存储为 f32，因为 Rust 没有原生 f16，但我们可以用 f32 模拟
    Float32(f32),
    Float64(f64),
    Bool(bool),
    Char(char),
    String(String),
    Unit,
    // ===== 新增：bytes"..." 字节字符串字面量，类型是 &[u8] =====
    // 内容规范上要求每字节都是合法 ASCII（0x00–0x7F），这条约束留给
    // parser（词法/语法层面检查）或 sema 去做，ast 这一层只负责装数据。
    ByteString(Vec<u8>),
}

// 关键修复：同 UnaryOp，补 Copy——simplify.rs 里 *op 解引用取值需要。
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

// ===== 类型系统（完整版本） =====
#[derive(Debug, PartialEq, Clone)]
pub enum Type {
    // 有符号整数
    I8, I16, I32, I64, I128,
    // 无符号整数
    U8, U16, U32, U64, U128,
    // 浮点数
    F16, F32, F64,
    SymInt,
    Bool,
    Char,
    Str,
    Struct(String),
    Enum(String),
    TypeParam(String),
    Generic(String, Vec<Type>),
    Tensor {
        dtype: Box<Type>,
        shape: Vec<ShapeDim>,
    },
    Privacy(Box<Type>, PrivacyTag),
    SelfType,
    ConstIntArray(Vec<i64>),
    // ===== 新增：元组类型 =====
    // 之前这门语言完全没有元组这个概念——Type 里连 Tuple 变体都不存
    // 在。新增字面量/模式匹配对元组的支持，必须先有对应的运行时类型
    // 落点，不然"元组"就只是个没有类型的语法糖。跟 Struct(String) 不
    // 同，元组没有名字，直接内联存各个位置的类型。
    Tuple(Vec<Type>),
    // ===== 新增：定长数组类型 [T; N] =====
    // 跟已有的 Slice(Box<Type>) 是两回事：Slice 单独存在没有意义、必
    // 须靠 Ref 借用（见 Slice 的注释——`&[T]`），Array 是"整体可以被
    // 持有/传值的定长聚合"，不需要经过 Ref 才能作为一个值使用，语义上
    // 更接近 Rust 的 `[T; N]`。跟 ConstIntArray(Vec<i64>) 也是两回事：
    // ConstIntArray 存的是编译期已知的一串 i64 字面量（用于形状/常量
    // 泛型参数那类场景），Array 描述的是"元素类型是 T、长度是 N 的普
    // 通数组值的类型"，元素可以是任意 Type，不要求是整数、也不要求在
    // 这里把每个元素的值都存下来。
    Array(Box<Type>, usize),
    Ref {
        mutable: bool,
        inner: Box<Type>,
    },
    // ===== 新增：切片类型 [T]，跟已有的 Ref 组合表达 &[T] =====
    // 单独存在没有意义（这门语言里切片必须借用），实际写法永远是
    // `Ref{ mutable, inner: Box::new(Slice(T)) }`，但拆成两层而不是直接
    // 搞一个 `Type::SliceRef(bool, Box<Type>)`，是为了跟 Rust 的
    // `&[T]`/`&mut [T]` 结构保持一致，以后如果要支持裸切片（比如
    // Box<[T]> 那种场景）不用再改类型结构。
    Slice(Box<Type>),
    Unit,
    // ===== 新增：never 类型（底类型 ⊥）=====
    // 表示"不会正常产生值"的表达式的类型：`return`/`break` 表达式本身、
    // panic-only 的调用、以及没有 break 出口的 `loop { ... }` 都可以标
    // 成 Never。它是所有类型的子类型（能隐式转换/统一到任意其它类型），
    // 但这条"可以兼容任何类型"的统一规则属于类型检查算法，留给 sema 去
    // 实现——ast 这一层只负责能把 `never` 关键字解析出的类型记下来。
    // 之前 LackSlice 的注释里提到"禁止 T 是 never"，指的就是这个变体：
    // 那条约束也是 sema 检查，这里不做限制。
    Never,
}

impl Type {
    // 从类型中提取隐私标签（如果有的话）。放在 Type 自己身上而不是某个
    // pass 的私有函数里：“这个类型带不带隐私标签”本来就是 Type 的属性，
    // 写成方法语义更直接，也方便 hir_builder.rs 之外的地方（比如以后
    // elaborate.rs 要判断隐私标签）复用，不用各自再写一遍 match。
    pub fn privacy_tag(&self) -> Option<PrivacyTag> {
        match self {
            Type::Privacy(_, tag) => Some(tag.clone()),
            _ => None,
        }
    }
}

// ===== 隐私标签 =====
#[derive(Debug, PartialEq, Clone)]
pub enum PrivacyTag {
    Public,
    Private,
    Differential { eps: String, delta: Option<String> },
}

// ===== 形状维度 =====
#[derive(Debug, PartialEq, Clone)]
pub enum ShapeDim {
    Const(usize),
    Sym(String),
    Dyn,
}