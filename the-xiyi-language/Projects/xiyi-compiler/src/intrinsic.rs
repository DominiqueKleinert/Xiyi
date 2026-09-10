// intrinsic.rs
use crate::ast::{Literal, Type};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(usize)]
pub enum IntrinsicFn {
    Print,
    Panic,
    FromUtf8Unchecked,
    TensorCond,
    TensorWhileLoop,
    Linear,
    Conv2d,
    MaxPool2d,
    Flatten,
    Reshape,
    Relu,
    Dropout,
    LayerNorm,
    Sum,
}

impl IntrinsicFn {
    // 手动维护的全部变体列表，替代 std::mem::variant_count::<IntrinsicFn>()。
    //
    // 关键修复：variant_count 到现在（2026）还是 nightly-only 的 unstable
    // API（tracking issue #73662，标着 B-unstable，还挂着
    // S-tracking-design-concerns，没有稳定的迹象），需要
    // `#![feature(variant_count)]` 才能用。这个 crate 要在 stable 工具链
    // 上编译，用了它直接编译不过。
    //
    // 这里不用宏（比如 strum 那类 derive 宏）去自动生成变体数量或列表——
    // 一是不想为了这一件事引入额外依赖，二是自举以后我们自己的语言大概率
    // 也没有对应的反射/宏能力，Rust 这边的实现风格越"手写直白"，以后照着
    // 搬过去就越省事。所以选择手写这份列表：新增/删除 IntrinsicFn 变体时，
    // 这里、from_str、以及 build_fn_table 里的 insert 都要跟着改——忘改
    // 的话，多出来的变体会在 `FN_TABLE.get(name as usize)` 那里悄悄返回
    // None（数组越界访问用的是 `.get`，不会 panic），表现为"这个内建函数
    // 查不到"，而不是编译错误。这是手动维护无法完全消除的代价，只能靠这
    // 条注释和三处改动挨在一起提醒。
    pub const ALL: [IntrinsicFn; 14] = [
        Self::Print,
        Self::Panic,
        Self::FromUtf8Unchecked,
        Self::TensorCond,
        Self::TensorWhileLoop,
        Self::Linear,
        Self::Conv2d,
        Self::MaxPool2d,
        Self::Flatten,
        Self::Reshape,
        Self::Relu,
        Self::Dropout,
        Self::LayerNorm,
        Self::Sum,
    ];

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "print" => Some(Self::Print),
            "panic" => Some(Self::Panic),
            "from_utf8_unchecked" => Some(Self::FromUtf8Unchecked),
            "tensor.cond" => Some(Self::TensorCond),
            "tensor.while_loop" => Some(Self::TensorWhileLoop),
            "linear" => Some(Self::Linear),
            "conv2d" => Some(Self::Conv2d),
            "max_pool2d" => Some(Self::MaxPool2d),
            "flatten" => Some(Self::Flatten),
            "reshape" => Some(Self::Reshape),
            "relu" => Some(Self::Relu),
            "dropout" => Some(Self::Dropout),
            "layer_norm" => Some(Self::LayerNorm),
            "sum" => Some(Self::Sum),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(usize)]
pub enum IntrinsicConst {
    I8Max,
    I16Max,
    I32Max,
    I64Max,
    I128Max,
    U8Max,
    U16Max,
    U32Max,
    U64Max,
    U128Max,
    F32Infinity,
    F32Nan,
    F64Infinity,
    F64Nan,
}

impl IntrinsicConst {
    // 同 IntrinsicFn::ALL，手动维护，理由见那边的注释。
    pub const ALL: [IntrinsicConst; 14] = [
        Self::I8Max,
        Self::I16Max,
        Self::I32Max,
        Self::I64Max,
        Self::I128Max,
        Self::U8Max,
        Self::U16Max,
        Self::U32Max,
        Self::U64Max,
        Self::U128Max,
        Self::F32Infinity,
        Self::F32Nan,
        Self::F64Infinity,
        Self::F64Nan,
    ];

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "i8::MAX" => Some(Self::I8Max),
            "i16::MAX" => Some(Self::I16Max),
            "i32::MAX" => Some(Self::I32Max),
            "i64::MAX" => Some(Self::I64Max),
            "i128::MAX" => Some(Self::I128Max),
            "u8::MAX" => Some(Self::U8Max),
            "u16::MAX" => Some(Self::U16Max),
            "u32::MAX" => Some(Self::U32Max),
            "u64::MAX" => Some(Self::U64Max),
            "u128::MAX" => Some(Self::U128Max),
            "f32::INFINITY" => Some(Self::F32Infinity),
            "f32::NAN" => Some(Self::F32Nan),
            "f64::INFINITY" => Some(Self::F64Infinity),
            "f64::NAN" => Some(Self::F64Nan),
            _ => None,
        }
    }
}

const FN_COUNT: usize = IntrinsicFn::ALL.len();
const CONST_COUNT: usize = IntrinsicConst::ALL.len();

// ===== 内建函数元数据 =====

#[derive(Debug, Clone, PartialEq)]
pub enum IntrinsicParam {
    Value(Type),
    Ref(bool, Type),
    Fn(Vec<Type>, Box<Type>),
    VarArgs,
}

#[derive(Debug, Clone)]
pub struct IntrinsicSignature {
    pub params: Vec<IntrinsicParam>,
    pub return_type: Type,
}

#[derive(Debug, Clone)]
pub struct Intrinsic {
    pub kind: IntrinsicKind,
    pub allowed_in_model: bool,
    pub is_pure: bool,
    pub requires_unsafe: bool,
    pub signature: Option<IntrinsicSignature>,
    pub link_name: &'static str,
    pub doc: &'static str,
}

impl Intrinsic {
    pub fn diverges(&self) -> bool {
        // 改成显式 match，不用 matches! 宏（哪怕它是标准库自带的基础
        // 宏）——你说了不想用宏，这里干脆一并去掉，别留个例外。
        match &self.signature {
            Some(IntrinsicSignature { return_type: Type::Never, .. }) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrinsicKind {
    Function,
    Special,
}

// ===== 常量元数据 =====

#[derive(Debug, Clone)]
pub struct Constant {
    pub name: &'static str,
    pub ty: Type,
    pub value: Option<Literal>,
    pub doc: &'static str,
}

impl Constant {
    pub fn has_known_value(&self) -> bool {
        self.value.is_some()
    }
}

// ===== 注册表（数组 + OnceLock） =====

// 与 HashMap 方案不同：用数组替代 HashMap，长度由 IntrinsicFn::ALL /
// IntrinsicConst::ALL 这两份手写列表的长度决定（原因见上面 ALL 的注释：
// std::mem::variant_count 是 nightly-only 的 unstable API，不能用）。
// 查找从 O(1) 哈希变成 O(1) 直接索引，无哈希开销。
static FN_TABLE: OnceLock<[Option<Intrinsic>; FN_COUNT]> = OnceLock::new();
static CONST_TABLE: OnceLock<[Option<Constant>; CONST_COUNT]> = OnceLock::new();

fn build_fn_table() -> [Option<Intrinsic>; FN_COUNT] {
    let mut arr: [Option<Intrinsic>; FN_COUNT] = std::array::from_fn(|_| None);
    let mut insert = |name: IntrinsicFn, intrinsic: Intrinsic| {
        arr[name as usize] = Some(intrinsic);
    };

    // ---- 基础 IO ----
    insert(IntrinsicFn::Print, Intrinsic {
        kind: IntrinsicKind::Function,
        allowed_in_model: false,
        is_pure: false,
        requires_unsafe: false,
        signature: Some(IntrinsicSignature {
            params: vec![IntrinsicParam::Value(Type::Str)],
            return_type: Type::Unit,
        }),
        link_name: "xiyi::io::print",
        doc: "Prints a value to standard output",
    });
    insert(IntrinsicFn::Panic, Intrinsic {
        kind: IntrinsicKind::Function,
        allowed_in_model: false,
        is_pure: false,
        requires_unsafe: false,
        signature: Some(IntrinsicSignature {
            params: vec![IntrinsicParam::Value(Type::Str)],
            return_type: Type::Never,
        }),
        link_name: "core::panic",
        doc: "Panics with a given message",
    });
    insert(IntrinsicFn::FromUtf8Unchecked, Intrinsic {
        kind: IntrinsicKind::Function,
        allowed_in_model: true,
        is_pure: true,
        requires_unsafe: true,
        signature: Some(IntrinsicSignature {
            params: vec![IntrinsicParam::Ref(false, Type::Slice(Box::new(Type::U8)))],
            return_type: Type::Ref { mutable: false, inner: Box::new(Type::Str) },
        }),
        link_name: "core::str::from_utf8_unchecked",
        doc: "Converts &[u8] to &str without validation (unsafe)",
    });

    // ---- 图域控制流 ----
    insert(IntrinsicFn::TensorCond, Intrinsic {
        kind: IntrinsicKind::Special,
        allowed_in_model: true,
        is_pure: true,
        requires_unsafe: false,
        signature: None,
        link_name: "xiyi_tensor::cond",
        doc: "Dynamic conditional in graph domain",
    });
    insert(IntrinsicFn::TensorWhileLoop, Intrinsic {
        kind: IntrinsicKind::Special,
        allowed_in_model: true,
        is_pure: true,
        requires_unsafe: false,
        signature: None,
        link_name: "xiyi_tensor::while_loop",
        doc: "Dynamic while loop in graph domain",
    });

    // ---- 张量算子（签名复杂，由 sema 检查） ----
    for (name, link_name, doc) in [
        (IntrinsicFn::Linear, "xiyi_math::linear", "Linear transformation"),
        (IntrinsicFn::Conv2d, "xiyi_math::conv2d", "2D convolution"),
        (IntrinsicFn::MaxPool2d, "xiyi_math::max_pool2d", "2D max pooling"),
        (IntrinsicFn::Flatten, "xiyi_math::flatten", "Flatten tensor"),
        (IntrinsicFn::Reshape, "xiyi_math::reshape", "Reshape tensor"),
        (IntrinsicFn::Relu, "xiyi_math::relu", "ReLU activation"),
        (IntrinsicFn::Dropout, "xiyi_math::dropout", "Dropout"),
        (IntrinsicFn::LayerNorm, "xiyi_math::layer_norm", "Layer normalization"),
        (IntrinsicFn::Sum, "xiyi_math::sum", "Sum reduction"),
    ] {
        insert(name, Intrinsic {
            kind: IntrinsicKind::Function,
            allowed_in_model: true,
            is_pure: true,
            requires_unsafe: false,
            signature: None,
            link_name,
            doc,
        });
    }

    arr
}

fn build_const_table() -> [Option<Constant>; CONST_COUNT] {
    let mut arr: [Option<Constant>; CONST_COUNT] = std::array::from_fn(|_| None);
    let mut insert = |name: IntrinsicConst, constant: Constant| {
        arr[name as usize] = Some(constant);
    };

    // 注：所有常量的 value 目前都是 None，因为常量折叠尚未实现。
    // 等实现后，将 None 替换为对应的 Literal 值即可。
    insert(IntrinsicConst::I8Max, Constant {
        name: "i8::MAX",
        ty: Type::I8,
        value: None,
        doc: "Maximum value of i8",
    });
    insert(IntrinsicConst::I16Max, Constant {
        name: "i16::MAX",
        ty: Type::I16,
        value: None,
        doc: "Maximum value of i16",
    });
    insert(IntrinsicConst::I32Max, Constant {
        name: "i32::MAX",
        ty: Type::I32,
        value: None,
        doc: "Maximum value of i32",
    });
    insert(IntrinsicConst::I64Max, Constant {
        name: "i64::MAX",
        ty: Type::I64,
        value: None,
        doc: "Maximum value of i64",
    });
    insert(IntrinsicConst::I128Max, Constant {
        name: "i128::MAX",
        ty: Type::I128,
        value: None,
        doc: "Maximum value of i128",
    });
    insert(IntrinsicConst::U8Max, Constant {
        name: "u8::MAX",
        ty: Type::U8,
        value: None,
        doc: "Maximum value of u8",
    });
    insert(IntrinsicConst::U16Max, Constant {
        name: "u16::MAX",
        ty: Type::U16,
        value: None,
        doc: "Maximum value of u16",
    });
    insert(IntrinsicConst::U32Max, Constant {
        name: "u32::MAX",
        ty: Type::U32,
        value: None,
        doc: "Maximum value of u32",
    });
    insert(IntrinsicConst::U64Max, Constant {
        name: "u64::MAX",
        ty: Type::U64,
        value: None,
        doc: "Maximum value of u64",
    });
    insert(IntrinsicConst::U128Max, Constant {
        name: "u128::MAX",
        ty: Type::U128,
        value: None,
        doc: "Maximum value of u128",
    });
    insert(IntrinsicConst::F32Infinity, Constant {
        name: "f32::INFINITY",
        ty: Type::F32,
        value: None,
        doc: "Infinity for f32",
    });
    insert(IntrinsicConst::F32Nan, Constant {
        name: "f32::NAN",
        ty: Type::F32,
        value: None,
        doc: "NaN for f32",
    });
    insert(IntrinsicConst::F64Infinity, Constant {
        name: "f64::INFINITY",
        ty: Type::F64,
        value: None,
        doc: "Infinity for f64",
    });
    insert(IntrinsicConst::F64Nan, Constant {
        name: "f64::NAN",
        ty: Type::F64,
        value: None,
        doc: "NaN for f64",
    });

    arr
}

// ===== 公共 API =====
pub fn get_intrinsic(name: IntrinsicFn) -> Option<&'static Intrinsic> {
    FN_TABLE
        .get_or_init(build_fn_table)
        .get(name as usize)?
        .as_ref()
}

pub fn get_constant(name: IntrinsicConst) -> Option<&'static Constant> {
    CONST_TABLE
        .get_or_init(build_const_table)
        .get(name as usize)?
        .as_ref()
}

pub fn is_intrinsic(s: &str) -> bool {
    IntrinsicFn::from_str(s).is_some() || IntrinsicConst::from_str(s).is_some()
}

// ===== 从 mir_builder.rs 搬过来的内建调用识别逻辑 =====
//
// 这两个函数原来是 mir_builder.rs 的 build_expr_rvalue 里
// HirExprKind::Call 分支内联的两段代码。之所以能搬过来而不用把
// MirBuilder 整个搬过来、也不用让这个文件反过来认识 mir.rs 的类型：
// 这两段逻辑其实只需要"这次调用长什么样"（func/qualifier/is_method/
// 参数个数）和 MirBuilder 当时的两个状态位（in_forward/unsafe_depth），
// 不需要真的持有 &mut MirBuilder，也不产出任何 MIR 值——常量引用只
// 返回一个名字字符串（由调用方自己包成 MirPlace::Static），内建函数
// 检测只返回"是不是、是哪个"，真正构造 MirRvalue::Call 的代码留在
// mir_builder.rs（那部分对内建调用和普通调用是共用的，拆出来意义不大）。

/// 尝试把一次零参裸调用识别成内建关联常量的引用（比如 `i128::MAX`）。
/// 命中返回这个常量的规范名字（用来构造 MirPlace::Static），命中不了
/// （不满足零参裸调用的形状，或者名字根本不是已知常量）返回 None，
/// 调用方按普通调用继续处理——常量引用语法上就是一个裸名字，不可能
/// 同时是方法调用或带参数，所以先拿这两个条件筛一遍。
pub fn try_resolve_constant_ref(
    qualifier: &Option<String>,
    func: &str,
    is_method: bool,
    arg_count: usize,
) -> Option<&'static str> {
    if qualifier.is_some() || is_method || arg_count != 0 {
        return None;
    }
    let name = IntrinsicConst::from_str(func)?;
    get_constant(name).map(|c| c.name)
}

/// 校验并识别一次调用是不是真正的内建/固有函数。
/// - 不是内建函数（用户自定义函数/普通调用）→ `Ok((false, None))`，
///   调用方按普通函数调用继续处理。
/// - 是内建函数但触发了限制（model 块里用了不允许的副作用函数、
///   unsafe 函数在 unsafe 块外被调用）→ `Err(...)`。
/// - 是内建函数且检查通过 → `Ok((true, Some(名字)))`。
///
/// `in_forward`/`unsafe_depth` 是 MirBuilder 自己的构建期状态，这两条
/// 检查天生依赖它们，所以按值传进来，而不是把 MirBuilder 整个传进来
/// ——这个文件不需要认识 MirBuilder 长什么样。`expr_diverges` 同理，
/// 是调用方已经从 HIR 表达式的 `ty` 字段（`Type::Never`）算好的结果，
/// 传一个 bool 进来即可，不用为了这一条 debug 断言让这个文件认识
/// hir.rs 的类型。
pub fn resolve_intrinsic_call(
    qualifier: &Option<String>,
    func: &str,
    is_method: bool,
    in_forward: bool,
    unsafe_depth: usize,
    expr_diverges: bool,
) -> Result<(bool, Option<IntrinsicFn>), String> {
    if qualifier.is_some() || is_method {
        return Ok((false, None));
    }
    let name = match IntrinsicFn::from_str(func) {
        Some(n) => n,
        None => return Ok((false, None)),
    };
    let intrinsic = match get_intrinsic(name) {
        Some(i) => i,
        None => return Ok((false, None)),
    };

    if in_forward && !intrinsic.allowed_in_model {
        return Err(format!(
            "error[MD001]: side-effect `{}` is not allowed in model block (forward method)",
            func
        ));
    }
    if intrinsic.requires_unsafe && unsafe_depth == 0 {
        return Err(format!(
            "call to unsafe intrinsic `{}` requires an `unsafe` block or `verify unsafe`",
            func
        ));
    }

    // 交叉校验：sema 给这个调用表达式算出来的发散性和 intrinsic 注册表
    // 自己声明的签名，理论上必须给出一致的答案（比如 panic() 应该是
    // Type::Never，跟 Intrinsic::diverges() 一致）。只在 debug 构建里
    // 检查，正常运行零开销。
    debug_assert_eq!(
        expr_diverges,
        intrinsic.diverges(),
        "intrinsic `{}` 的签名声明的发散性跟 sema 算出的表达式类型对不上",
        func
    );

    Ok((true, Some(name)))
}