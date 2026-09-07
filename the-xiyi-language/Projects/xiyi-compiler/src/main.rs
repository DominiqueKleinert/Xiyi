use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use xiyi_compiler::{
    ast::Item,
    parser::Parser,
    sema::TypeChecker,
    borrow, control, monomorphic,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: xiyi [--stdlib <path>] <file.xiyi>");
        std::process::exit(1);
    }

    let mut filename = None;
    let mut stdlib_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--stdlib" => {
                if i + 1 < args.len() {
                    stdlib_path = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("--stdlib requires a path argument");
                    std::process::exit(1);
                }
            }
            _ => {
                if filename.is_none() && !args[i].starts_with("--") {
                    filename = Some(args[i].clone());
                }
                i += 1;
            }
        }
    }

    let filename = match filename {
        Some(f) => f,
        None => {
            eprintln!("No input file specified");
            std::process::exit(1);
        }
    };

    // ===== stdlib 路径解析 =====
    // 优先使用 --stdlib 显式传入的路径；
    // 未传入时，不再写死一个假设"当前工作目录就是 xiyi-compiler 根目录"的
    // 相对路径（那正是目录重排那次事故的病因），而是从可执行文件自身的
    // 位置反推工作区根目录，再拼上 Standard/。
    // 找不到时才退化为旧的 "Standard/" 相对路径兜底，并明确提示用户，
    // 避免静默用错目录导致一堆看似无关的类型错误。
    let stdlib_path = match stdlib_path {
        Some(p) => p,
        None => match default_stdlib_path() {
            Some(p) => {
                eprintln!(
                    "未指定 --stdlib，使用推断出的标准库路径: {}",
                    p.display()
                );
                p.to_string_lossy().into_owned()
            }
            None => {
                eprintln!(
                    "警告：未指定 --stdlib，且无法从可执行文件位置推断标准库目录，\
                     回退使用相对路径 \"Standard/\"（很可能找不到，请显式传入 --stdlib）"
                );
                String::from("Standard/")
            }
        },
    };

    // 1. 加载用户源码
    let source = match fs::read_to_string(&filename) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file {}: {}", filename, e);
            std::process::exit(1);
        }
    };

    // 2. 加载标准库（带来源追踪，用于冲突检测）
    let stdlib_items = match load_stdlib(&stdlib_path) {
        Ok(items) => items,
        Err(e) => {
            eprintln!("Failed to load standard library: {}", e);
            std::process::exit(1);
        }
    };

    // 3. 解析用户源码
    let mut parser = Parser::new(&source);
    let user_program = match parser.parse_program() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    };

    // 4. 合并标准库和用户程序（带命名冲突检测）
    let program = match merge_with_conflict_check(stdlib_items, user_program.items, &filename) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // 5. 类型检查 + HIR
    let mut checker = TypeChecker::new();
    let hir = match checker.check_program(&program) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Type error: {}", e);
            std::process::exit(1);
        }
    };

    // 6. 展开语法糖（for、? 等）
    let hir = match xiyi_compiler::elaborate::Elaborate::elaborate(hir) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Elaboration error: {}", e);
            std::process::exit(1);
        }
    };

    // 7. 构建 MIR
    let mir = match xiyi_compiler::mir_builder::MirBuilder::build(&hir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("MIR build error: {}", e);
            std::process::exit(1);
        }
    };

    // 8. 控制流简化（先清理 CFG）
    let mut mir = mir;
    control::Control::simplify(&mut mir);

    // 9. 借用检查（在干净的 CFG 上跑）
    if let Err(e) = borrow::BorrowChecker::check(&mir) {
        eprintln!("Borrow check error: {}", e);
        std::process::exit(1);
    }

    // 10. 单态化（展开泛型）
    let mir = monomorphic::Monomorphic::run(mir);

    // 11. MIR 优化（常量折叠、死代码消除）
    let mir = xiyi_compiler::simplify::Simplify::run(mir);

    // 12. 代码生成
    let rust_code = xiyi_compiler::codegen::Codegen::generate_from_mir(&mir);

    // 13. 构建输出目录并编译
    // 不再写死 Windows 专属的 "D:\xiyi_build"：
    // 一是跨平台直接失效（no_std / WujiOS 目标机器上没有 D 盘概念），
    // 二是团队里未必人人都有可写的 D 盘。改用系统临时目录，
    // 在所有平台上都能正确解析，且同一台机器上的多次运行仍共享同一个
    // 构建目录，保留 cargo 的增量编译缓存收益。
    let temp_dir = std::env::temp_dir().join("xiyi_build");
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        eprintln!("Failed to create temp dir: {}", e);
        std::process::exit(1);
    }

    // 注释掉 tch 依赖，避免 torch-sys 编译失败
    let cargo_toml = r#"[package]
name = "xiyi_output"
version = "0.1.0"
edition = "2024"

[dependencies]
# tch = { version = "0.13", features = ["download-libtorch"] }

[[bin]]
name = "xiyi_output"
path = "src/main.rs"
"#;
    if let Err(e) = fs::write(temp_dir.join("Cargo.toml"), cargo_toml) {
        eprintln!("Failed to write Cargo.toml: {}", e);
        std::process::exit(1);
    }

    let src_dir = temp_dir.join("src");
    if let Err(e) = fs::create_dir_all(&src_dir) {
        eprintln!("Failed to create src dir: {}", e);
        std::process::exit(1);
    }
    if let Err(e) = fs::write(src_dir.join("main.rs"), rust_code) {
        eprintln!("Failed to write main.rs: {}", e);
        std::process::exit(1);
    }

    // 14. cargo build
    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&temp_dir)
        .status();

    match status {
        Ok(status) if status.success() => {
            let exe_path = if cfg!(windows) {
                temp_dir.join("target").join("debug").join("xiyi_output.exe")
            } else {
                temp_dir.join("target").join("debug").join("xiyi_output")
            };
            if !exe_path.exists() {
                eprintln!("Executable not found after build");
                std::process::exit(1);
            }
            let run_status = Command::new(exe_path).status();
            match run_status {
                Ok(status) => std::process::exit(status.code().unwrap_or(0)),
                Err(e) => {
                    eprintln!("Failed to run executable: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Ok(_) => {
            eprintln!("Cargo build failed");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to run cargo: {}", e);
            std::process::exit(1);
        }
    }
}

/// 从可执行文件自身的位置反推工作区根目录下的 Standard 目录。
///
/// 约定的目录结构（与 run_all_tests.ps1 保持一致）：
///   <workspace_root>/
///   ├── Projects/xiyi-compiler/target/{debug,release}/xiyi[.exe]  <- 当前可执行文件
///   └── Standard/                                                  <- 要找的目录
///
/// 即从 exe 往上数 5 层目录：
///   {debug,release} -> target -> xiyi-compiler -> Projects -> <workspace_root>
///
/// 找不到 current_exe()、路径层级不够深、或者推断出的 Standard 目录
/// 实际不存在时，一律返回 None，交给调用方决定是否兜底。
fn default_stdlib_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;

    // exe 所在目录 (target/debug 或 target/release)
    let debug_or_release_dir = exe.parent()?;
    let target_dir = debug_or_release_dir.parent()?;
    let compiler_root = target_dir.parent()?; // .../Projects/xiyi-compiler
    let projects_dir = compiler_root.parent()?; // .../Projects
    let workspace_root = projects_dir.parent()?; // .../the-xiyi-language

    let candidate = workspace_root.join("Standard");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

fn item_name(item: &Item) -> Option<&str> {
    match item {
        Item::FnDef(f) => Some(&f.name),
        Item::StructDef(s) => Some(&s.name),
        Item::EnumDef(e) => Some(&e.name),
        Item::ConstDef(c) => Some(&c.name),
        Item::ModelDef(m) => Some(&m.name),
        Item::ProtoDef(p) => Some(&p.name),
        Item::Interface(iface) => Some(&iface.name),
        Item::Use(_) => None,
        Item::Implement(_) => None,
    }
}

fn load_stdlib(stdlib_path: &str) -> Result<Vec<Item>, String> {
    let core_src = Path::new(stdlib_path).join("xiyi-core").join("src");

    if !core_src.is_dir() {
        return Err(format!(
            "stdlib source directory not found: {}",
            core_src.display()
        ));
    }

    // 收集所有 .xiyi 文件，lib.xiyi 单独放到最后加载
    let mut module_files: Vec<std::path::PathBuf> = Vec::new();
    let mut lib_file: Option<std::path::PathBuf> = None;

    let entries = fs::read_dir(&core_src)
        .map_err(|e| format!("Failed to read {}: {}", core_src.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read dir entry: {}", e))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("xiyi") {
            continue;
        }
        if path.file_name().and_then(|s| s.to_str()) == Some("lib.xiyi") {
            lib_file = Some(path);
        } else {
            module_files.push(path);
        }
    }
    // 按文件名排序，保证跨平台、跨运行的加载顺序确定
    module_files.sort();
    if let Some(lib) = lib_file {
        module_files.push(lib);
    }

    if module_files.is_empty() {
        // 允许空标准库，返回空 Vec
        return Ok(Vec::new());
    }

    let mut all_items: Vec<Item> = Vec::new();
    // 记录每个已定义名字来自哪个模块文件，用于冲突检测和报错定位
    let mut defined_in: HashMap<String, String> = HashMap::new();

    for file_path in &module_files {
        let module_label = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();

        let source = fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

        let mut parser = Parser::new(&source);
        let program = parser
            .parse_program()
            .map_err(|e| format!("Parse error in {}: {}", module_label, e))?;

        for item in program.items {
            if let Some(name) = item_name(&item) {
                if let Some(prev_module) = defined_in.get(name) {
                    return Err(format!(
                        "duplicate definition `{}` in stdlib: defined in both `{}` and `{}`",
                        name, prev_module, module_label
                    ));
                }
                defined_in.insert(name.to_string(), module_label.clone());
            }
            all_items.push(item);
        }
    }

    Ok(all_items)
}

fn merge_with_conflict_check(
    stdlib_items: Vec<Item>,
    user_items: Vec<Item>,
    user_filename: &str,
) -> Result<xiyi_compiler::ast::Program, String> {
    let stdlib_names: std::collections::HashSet<String> = stdlib_items
        .iter()
        .filter_map(item_name) // 只保留有名字的
        .map(|s| s.to_string())
        .collect();

    for item in &user_items {
        if let Some(name) = item_name(item) {
            if stdlib_names.contains(name) {
                return Err(format!(
                    "error: `{}` in {} conflicts with a standard library definition of the same name.",
                    name, user_filename
                ));
            }
        }
    }

    let mut all_items = stdlib_items;
    all_items.extend(user_items);
    Ok(xiyi_compiler::ast::Program { items: all_items })
}
