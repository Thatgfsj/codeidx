//! 单测：纯函数（分帧 / URI / 解析 / 映射）+ rust-analyzer 实机端到端
//! （F3.1–F3.5 同型断言）。
//!
//! 实机部分共用一个 `#[tokio::test]` 顺序执行，避免多个 rust-analyzer
//! 并发冷启动；fixture 一律使用副本（cargo metadata 会写 Cargo.lock），
//! 不污染仓库目录。测试内禁止向 stdout 打印（tracing 走 stderr）。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use codeidx_ast::Lang;
use serde_json::json;
use tokio::io::{duplex, AsyncWriteExt, BufReader};

use crate::detect::{detect_server, language_id};
use crate::proto;
use crate::{Location, LspBridge, LspError};

// ---------------------------------------------------------------- 纯单测 --

#[tokio::test]
async fn frame_round_trip() {
    let (mut writer, raw) = duplex(4096);
    let mut reader = BufReader::new(raw);
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"ok": true}});
    proto::write_frame(&mut writer, &msg).await.unwrap();
    let back = proto::read_frame(&mut reader).await.unwrap().unwrap();
    assert_eq!(back, msg);
}

#[tokio::test]
async fn frame_eof_returns_none() {
    let (writer, raw) = duplex(64);
    drop(writer);
    let mut reader = BufReader::new(raw);
    assert!(proto::read_frame(&mut reader).await.unwrap().is_none());
}

#[tokio::test]
async fn frame_missing_content_length_is_error() {
    let (mut writer, raw) = duplex(64);
    writer.write_all(b"X-Custom: 1\r\n\r\n").await.unwrap();
    writer.flush().await.unwrap();
    let mut reader = BufReader::new(raw);
    assert!(proto::read_frame(&mut reader).await.is_err());
}

#[cfg(unix)]
#[test]
fn uri_round_trip_preserves_path() {
    for path in ["/tmp/plain/src/lib.rs", "/tmp/a b/项目 100%.rs"] {
        let uri = proto::path_to_uri(Path::new(path));
        assert!(uri.starts_with("file://"));
        assert_eq!(proto::uri_to_path(&uri), Some(PathBuf::from(path)));
    }
}

#[cfg(unix)]
#[test]
fn uri_to_display_path_prefers_root_relative() {
    let root = Path::new("/tmp/proj");
    let rel = proto::uri_to_display_path("file:///tmp/proj/src/lib.rs", root);
    assert_eq!(rel.as_deref(), Some("src/lib.rs"));
    // 项目外目标（stdlib / 依赖源码）保留绝对路径，不静默丢弃。
    let outside = proto::uri_to_display_path("file:///usr/src/lib.rs", root);
    assert_eq!(outside.as_deref(), Some("/usr/src/lib.rs"));
}

#[test]
fn parse_locations_variants() {
    #[cfg(not(windows))]
    let (root, uri_prefix) = (Path::new("/tmp/proj"), "file:///tmp/proj");
    #[cfg(windows)]
    let (root, uri_prefix) = (Path::new(r"\\?\C:\proj"), "file:///c:/proj");

    let null = json!(null);
    assert!(proto::parse_locations(&null, root).is_empty());

    let single = json!({
        "uri": format!("{uri_prefix}/src/lib.rs"),
        "range": {"start": {"line": 26, "character": 7}},
    });
    assert_eq!(
        proto::parse_locations(&single, root),
        vec![Location {
            file: "src/lib.rs".into(),
            line: 27,
            character: 7,
        }]
    );

    // LocationLink 数组：优先 targetSelectionRange。
    let links = json!([{
        "targetUri": format!("{uri_prefix}/src%20copy/lib.rs"),
        "targetRange": {"start": {"line": 10, "character": 0}},
        "targetSelectionRange": {"start": {"line": 9, "character": 4}},
    }]);
    assert_eq!(
        proto::parse_locations(&links, root),
        vec![Location {
            file: "src copy/lib.rs".into(),
            line: 10,
            character: 4,
        }]
    );
}

#[cfg(windows)]
#[test]
fn windows_file_uris_round_trip() {
    let file = r"C:\a b\项目 100% #.rs";
    let unc = r"\\server\share\src\lib.rs";
    for (original, uri, normal) in [
        (
            file,
            "file:///C:/a%20b/%E9%A1%B9%E7%9B%AE%20100%25%20%23.rs",
            file,
        ),
        (
            r"\\?\C:\a b\项目 100% #.rs",
            "file:///C:/a%20b/%E9%A1%B9%E7%9B%AE%20100%25%20%23.rs",
            file,
        ),
        (unc, "file://server/share/src/lib.rs", unc),
        (
            r"\\?\UNC\server\share\src\lib.rs",
            "file://server/share/src/lib.rs",
            unc,
        ),
    ] {
        assert_eq!(proto::path_to_uri(Path::new(original)), uri);
        assert_eq!(proto::uri_to_path(uri), Some(PathBuf::from(normal)));
    }
    assert_eq!(
        proto::uri_to_path("file://localhost/C:/"),
        Some(PathBuf::from(r"C:\"))
    );
    assert_eq!(
        proto::uri_to_path("file:///C%3A/src/lib.rs"),
        Some(PathBuf::from(r"C:\src\lib.rs"))
    );
    assert_eq!(proto::uri_to_path("https://server/share/src/lib.rs"), None);
    assert_eq!(proto::uri_to_path("file://server/"), None);
    assert_eq!(
        proto::uri_to_display_path(
            "file://server/share/src/lib.rs",
            Path::new(r"\\?\UNC\server\share")
        )
        .as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(
        proto::uri_to_display_path("file:///D:/src/lib.rs", Path::new(r"\\?\C:\src")).as_deref(),
        Some(r"D:\src\lib.rs")
    );
}

#[test]
fn windows_pathext_filters_non_launchable_suffixes_in_order() {
    use crate::detect::windows_extensions;

    assert_eq!(
        windows_extensions(None).collect::<Vec<_>>(),
        [".com", ".exe", ".bat", ".cmd"]
    );
    assert_eq!(
        windows_extensions(Some("")).collect::<Vec<_>>(),
        [".com", ".exe", ".bat", ".cmd"]
    );
    assert_eq!(
        windows_extensions(Some(".PY; .cMd ;.ps1;;.EXE;.TXT;.BAT;.CoM")).collect::<Vec<_>>(),
        [".cmd", ".exe", ".bat", ".com"]
    );
}

#[cfg(any(unix, windows))]
#[test]
fn resolve_server_uses_absolute_usable_path_in_path_order() {
    use crate::detect::resolve_on_path;

    let root = std::env::temp_dir().join(format!("codeidx-lsp-resolve-{}", std::process::id()));
    let earlier = root.join("earlier bin");
    let later = root.join("later bin");
    fs::create_dir_all(&earlier).unwrap();
    fs::create_dir_all(&later).unwrap();
    let search = std::env::join_paths([&earlier, &later]).unwrap();
    let name = "rust-analyzer";

    #[cfg(windows)]
    {
        fs::write(earlier.join(name), "").unwrap();
        fs::write(earlier.join("rust-analyzer.py"), "").unwrap();
        fs::create_dir(earlier.join("rust-analyzer.exe")).unwrap();
        let exe = later.join("rust-analyzer.exe");
        let cmd = later.join("rust-analyzer.cmd");
        fs::write(&exe, "").unwrap();
        fs::write(&cmd, "@exit /b 17\r\n").unwrap();
        assert_eq!(
            resolve_on_path(name, &search, Some(".PY;.EXE;.CMD")),
            Some(exe)
        );
        let program = resolve_on_path(name, &search, Some(".CMD;.EXE")).unwrap();
        assert!(program.is_absolute());
        assert_eq!(program, cmd);
        assert_eq!(
            std::process::Command::new(program).status().unwrap().code(),
            Some(17)
        );
        assert_eq!(resolve_on_path(name, &search, Some(".PY;.TXT")), None);
        let first = earlier.join("rust-analyzer.cmd");
        fs::write(&first, "").unwrap();
        assert_eq!(
            resolve_on_path(name, &search, Some(".EXE;.CMD")),
            Some(first)
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let non_executable = earlier.join(name);
        fs::write(&non_executable, "").unwrap();
        fs::set_permissions(&non_executable, fs::Permissions::from_mode(0o644)).unwrap();
        let real_file = later.join("target");
        fs::write(&real_file, "#!/bin/sh\nexit 17\n").unwrap();
        fs::set_permissions(&real_file, fs::Permissions::from_mode(0o755)).unwrap();
        let link = later.join(name);
        symlink(&real_file, &link).unwrap();
        let program = resolve_on_path(name, &search, None).unwrap();
        assert!(program.is_absolute());
        assert_eq!(program, link);
        assert_eq!(
            std::process::Command::new(program).status().unwrap().code(),
            Some(17)
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parse_hover_variants() {
    assert_eq!(proto::parse_hover(&json!(null)), "");
    let markup = json!({"contents": {"kind": "plaintext", "value": "pub fn f()"}});
    assert_eq!(proto::parse_hover(&markup), "pub fn f()");
    let marked = json!({"contents": [{"language": "rust", "value": "fn a()"}, "plain"]});
    assert_eq!(proto::parse_hover(&marked), "fn a()\nplain");
}

#[test]
fn language_id_mapping() {
    assert_eq!(language_id(Lang::Rust), "rust");
    assert_eq!(language_id(Lang::TypeScript), "typescript");
    assert_eq!(language_id(Lang::JavaScript), "javascript");
    assert_eq!(language_id(Lang::Python), "python");
    assert_eq!(language_id(Lang::Go), "go");
    assert_eq!(language_id(Lang::Java), "java");
    assert_eq!(language_id(Lang::C), "c");
    assert_eq!(language_id(Lang::Cpp), "cpp");
}

#[test]
fn lsp_error_display_and_source() {
    let err = LspError("boom".into());
    assert_eq!(err.to_string(), "boom");
    fn takes_error(_: &dyn std::error::Error) -> bool {
        true
    }
    assert!(takes_error(&err));
}

// ------------------------------------------------------------ 实机端到端 --

/// fixture 源目录（仓库内只读；实测一律用副本）。
fn fixture_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-project")
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "__pycache__" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// 文本中首个包含 needle（且含 extra，extra 为空则忽略）的
/// (1-based 行, 0-based 列)。
fn find_pos(text: &str, needle: &str, extra: &str) -> (u32, u32) {
    for (i, line) in text.lines().enumerate() {
        if !line.contains(needle) {
            continue;
        }
        if !extra.is_empty() && !line.contains(extra) {
            continue;
        }
        if let Some(col) = line.find(needle) {
            return ((i + 1) as u32, col as u32);
        }
    }
    panic!("marker {needle:?} not found in fixture");
}

/// needle 在指定 1-based 行内的 0-based 列。
fn column_in_line(text: &str, line_1based: u32, needle: &str) -> u32 {
    text.lines()
        .nth(line_1based as usize - 1)
        .unwrap_or_else(|| panic!("line {line_1based} out of range"))
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in line {line_1based}")) as u32
}

/// 含 needle 的所有 1-based 行号。
fn line_numbers_containing(text: &str, needle: &str) -> BTreeSet<u32> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| (i + 1) as u32)
        .collect()
}

/// ps 等价验证：/proc 中 comm 为 rust-analyzer 且父进程为本测试进程的
/// pid 集合（应为空 = 无孤儿 / 无残留）。
fn rust_analyzer_children() -> Vec<u32> {
    let me = std::process::id();
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        if comm.trim() != "rust-analyzer" {
            continue;
        }
        let Ok(status) = fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let is_child = status.lines().any(|l| {
            l.strip_prefix("PPid:")
                .map(|v| v.trim().parse::<u32>().ok() == Some(me))
                .unwrap_or(false)
        });
        if is_child {
            found.push(pid);
        }
    }
    found
}

/// 唯一的 rust-analyzer 实机测试：definition → references → hover →
/// didChange → idle 回收 → kill_for_root 顺序执行（单进程串行，避免并发
/// 冷启动；F3.1–F3.5 同型断言）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_analyzer_end_to_end() {
    // fixture 副本：cargo metadata 会写 Cargo.lock，绝不污染仓库 fixture。
    let root = std::env::temp_dir().join(format!("codeidx-lsp-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    copy_dir(&fixture_src(), &root);

    let main_rel = "src/main.rs";
    let lib_rel = "src/lib.rs";
    let utils_rel = "src/utils.rs";
    let main_text = fs::read_to_string(root.join(main_rel)).unwrap();
    let lib_text = fs::read_to_string(root.join(lib_rel)).unwrap();
    let utils_text = fs::read_to_string(root.join(utils_rel)).unwrap();

    // 动态定位并与 fixture 手工标注交叉验证。
    let (call_line, call_char) = find_pos(&main_text, "unicorn_marker_fn", "(&cfg)");
    assert_eq!(call_line, 9, "main.rs 调用点手工标注 = 第 9 行");
    let (def_line, _) = find_pos(&lib_text, "pub fn unicorn_marker_fn", "");
    assert_eq!(def_line, 27, "lib.rs 定义手工标注 = 第 27 行");
    let def_char = column_in_line(&lib_text, def_line, "unicorn_marker_fn");
    assert_eq!(def_char, 7);
    let (helper_def_line, _) = find_pos(&utils_text, "pub fn helper_spinner", "");
    assert_eq!(helper_def_line, 4, "utils.rs 定义手工标注 = 第 4 行");
    let helper_def_char = column_in_line(&utils_text, helper_def_line, "helper_spinner");
    let expected_lib_ref_lines = line_numbers_containing(&lib_text, "helper_spinner(");
    assert_eq!(expected_lib_ref_lines, BTreeSet::from([28, 29, 30]));

    let bridge = Arc::new(LspBridge::new(2));

    // F3.1 definition：main.rs 调用点 → lib.rs 定义行（1-based）。
    let defs = bridge
        .definition(
            &root,
            Lang::Rust,
            main_rel,
            call_line,
            call_char,
            &main_text,
        )
        .await
        .unwrap();
    let hit = defs
        .iter()
        .find(|l| l.file == lib_rel && l.line == def_line)
        .unwrap_or_else(|| panic!("应跳到 {lib_rel}:{def_line}，实测 {defs:?}"));
    assert_eq!(
        hit.character, def_char,
        "选中范围为符号名起点（0-based char）"
    );
    assert!(defs.iter().all(|l| l.line >= 1), "对外行号 1-based");

    // F3.2 references：utils.rs 定义点 → lib.rs 内恰 3 处引用。
    let refs = bridge
        .references(
            &root,
            Lang::Rust,
            utils_rel,
            helper_def_line,
            helper_def_char,
            &utils_text,
        )
        .await
        .unwrap();
    let lib_ref_lines: BTreeSet<u32> = refs
        .iter()
        .filter(|l| l.file == lib_rel)
        .map(|l| l.line)
        .collect();
    assert_eq!(
        lib_ref_lines, expected_lib_ref_lines,
        "lib.rs 内引用恰为 28/29/30 行，实测 {refs:?}"
    );
    assert!(refs.len() >= 3, "任务断言：lib.rs 内 ≥3 处引用");

    // hover：定义点 → 非空 contents 且点名符号。
    let hover = bridge
        .hover(&root, Lang::Rust, lib_rel, def_line, def_char, &lib_text)
        .await
        .unwrap();
    assert!(!hover.is_empty(), "hover contents 非空");
    assert!(
        hover.contains("unicorn_marker_fn"),
        "hover 应包含符号名，实测: {hover}"
    );

    // didChange 全量替换：内容变化后仍能正确应答同一跳转。
    let main_text_v2 = format!("{main_text}\n// trailing comment\n");
    let defs2 = bridge
        .definition(
            &root,
            Lang::Rust,
            main_rel,
            call_line,
            call_char,
            &main_text_v2,
        )
        .await
        .unwrap();
    assert!(
        defs2
            .iter()
            .any(|l| l.file == lib_rel && l.line == def_line),
        "didChange 后仍应跳到 {lib_rel}:{def_line}，实测 {defs2:?}"
    );

    // F3.3 idle：reaper + 2s 阈值 + 5s 等待 → 进程被 SIGKILL（ps 验证）。
    let reaper = bridge.spawn_reaper();
    // 触发一次调用重置 last_used，从此刻起计 2s 空闲。
    let _ = bridge
        .hover(&root, Lang::Rust, lib_rel, def_line, def_char, &lib_text)
        .await
        .unwrap();
    assert_eq!(bridge.active_lsps(&root), vec!["rust-analyzer".to_owned()]);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        bridge.active_lsps(&root).is_empty(),
        "idle 超时后进程应被回收"
    );
    assert!(
        rust_analyzer_children().is_empty(),
        "ps 验证：rust-analyzer 子进程已 kill，残留 {:?}",
        rust_analyzer_children()
    );
    reaper.abort();

    // F3.4 kill_for_root：重新拉起后同步 kill，无孤儿（ps 验证）。
    let _ = bridge
        .definition(
            &root,
            Lang::Rust,
            main_rel,
            call_line,
            call_char,
            &main_text_v2,
        )
        .await
        .unwrap();
    assert_eq!(bridge.active_lsps(&root).len(), 1, "重新按需拉起");
    let killed = bridge.kill_for_root(&root);
    assert_eq!(killed, 1, "kill_for_root 返回 kill 数");
    assert!(bridge.active_lsps(&root).is_empty());
    assert!(
        rust_analyzer_children().is_empty(),
        "ps 验证：kill_for_root 后无孤儿 rust-analyzer"
    );

    // 清理副本。
    fs::remove_dir_all(&root).unwrap();
}

/// F3.5 对型：未安装语言服务器 → 契约错误文案，不崩溃、不拉进程。
#[tokio::test]
async fn missing_language_server_is_a_clean_error() {
    // 验收机无 gopls（可用性检查已确认）；装有 gopls 的环境跳过。
    if detect_server(Lang::Go).is_ok() {
        return;
    }
    let root = std::env::temp_dir().join(format!("codeidx-lsp-go-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let bridge = LspBridge::new(300);
    let err = bridge
        .definition(&root, Lang::Go, "main.go", 1, 0, "package main\n")
        .await
        .unwrap_err();
    assert!(
        err.0.contains("not found on PATH"),
        "契约文案应含 not found on PATH，实测: {}",
        err.0
    );
    assert!(err.0.contains("go"), "错误应点名语言，实测: {}", err.0);
    assert!(bridge.active_lsps(&root).is_empty());
    let _ = fs::remove_dir_all(&root);
}

/// F3.6/F3.7 对型：basedpyright 与 gopls 在 polyglot fixture 副本上的
/// 多语言语义实测；服务器缺失的环境自动跳过（探测失败即跳）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polyglot_python_and_go_semantics() {
    let root = std::env::temp_dir().join(format!("codeidx-lsp-poly-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/polyglot-project"),
        &root,
    );

    let app_rel = "app.py";
    let models_rel = "models.py";
    let main_rel = "main.go";
    let types_rel = "types.go";
    let app_text = fs::read_to_string(root.join(app_rel)).unwrap();
    let models_text = fs::read_to_string(root.join(models_rel)).unwrap();
    let main_text = fs::read_to_string(root.join(main_rel)).unwrap();
    let types_text = fs::read_to_string(root.join(types_rel)).unwrap();

    // 动态定位锚点并与 fixture 手工标注交叉验证。
    let (py_call_line, py_call_char) = find_pos(&app_text, "GoldWidget(", "");
    assert_eq!(py_call_line, 7, "app.py 构造调用行");
    let (py_class_line, _) = find_pos(&models_text, "class GoldWidget", "");
    assert_eq!(py_class_line, 4, "models.py 类定义行");
    // references 光标必须落在类名上（列 6），落在 class 关键字上会得到空集。
    let py_class_char = column_in_line(&models_text, py_class_line, "GoldWidget");
    assert_eq!(py_class_char, 6);
    let (go_call_line, go_call_char) = find_pos(&main_text, "NewWidget(", "");
    assert_eq!(go_call_line, 7, "main.go 构造调用行");
    let (go_def_line, _) = find_pos(&types_text, "func NewWidget", "");
    assert_eq!(go_def_line, 13, "types.go NewWidget 定义行");
    let go_method_char = column_in_line(&types_text, 18, "Describe");
    let go_use_line = find_pos(&main_text, "w.Describe()", "").0;
    assert_eq!(go_use_line, 12, "main.go Describe 引用行");
    let py_hover_char = column_in_line(&models_text, 11, "describe");

    let bridge = Arc::new(LspBridge::new(300));

    // Python / basedpyright：definition 构造调用 → 类定义；references
    // 引用集（import / 构造 / 注解 ≥3）；hover 方法签名。
    if detect_server(Lang::Python).is_ok() {
        let defs = bridge
            .definition(
                &root,
                Lang::Python,
                app_rel,
                py_call_line,
                py_call_char,
                &app_text,
            )
            .await
            .unwrap();
        assert!(
            defs.iter()
                .any(|l| l.file == models_rel && l.line == py_class_line),
            "py definition 应跳 models.py:{py_class_line}，实测 {defs:?}"
        );
        let refs = bridge
            .references(
                &root,
                Lang::Python,
                models_rel,
                py_class_line,
                py_class_char,
                &models_text,
            )
            .await
            .unwrap();
        assert!(
            refs.len() >= 3,
            "类引用应含 import/构造/注解 ≥3 处，实测 {refs:?}"
        );
        let hover = bridge
            .hover(
                &root,
                Lang::Python,
                models_rel,
                11,
                py_hover_char,
                &models_text,
            )
            .await
            .unwrap();
        assert!(
            !hover.is_empty() && hover.contains("describe"),
            "hover 应含方法签名，实测: {hover}"
        );
    }

    // Go / gopls：definition 构造调用 → 构造函数定义；references 方法引用。
    if detect_server(Lang::Go).is_ok() {
        let defs = bridge
            .definition(
                &root,
                Lang::Go,
                main_rel,
                go_call_line,
                go_call_char,
                &main_text,
            )
            .await
            .unwrap();
        assert!(
            defs.iter()
                .any(|l| l.file == types_rel && l.line == go_def_line),
            "go definition 应跳 types.go:{go_def_line}，实测 {defs:?}"
        );
        let refs = bridge
            .references(&root, Lang::Go, types_rel, 18, go_method_char, &types_text)
            .await
            .unwrap();
        assert!(
            refs.iter()
                .any(|l| l.file == main_rel && l.line == go_use_line),
            "go references 应含 main.go:{go_use_line}，实测 {refs:?}"
        );
    }

    // 生命周期：回收已拉起的服务器（0/1/2 取决于环境安装情况）。
    let killed = bridge.kill_for_root(&root);
    assert!(killed <= 2, "至多 py + go 两个进程");
    fs::remove_dir_all(&root).unwrap();
}
