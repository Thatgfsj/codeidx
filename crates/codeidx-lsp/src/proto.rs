//! LSP wire 协议最小实现：`Content-Length` 分帧的 JSON-RPC 读写、
//! `file://` URI 转换，以及 definition / references / hover 响应到
//! 对外 [`Location`] 与纯文本的解析。
//!
//! 行号约定：对外一律 1-based line、0-based character（UTF-16 code
//! unit）；LSP wire 上两者均 0-based，转换只发生在桥层。

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// 对外统一的定位：项目根相对路径 + 1-based 行 + 0-based 列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// 项目根相对路径（`/` 分隔）。
    pub file: String,
    /// 行（1-based）。
    pub line: u32,
    /// 列（0-based，UTF-16 code unit）。
    pub character: u32,
}

/// 桥层错误：字符串原因，上层可直接透传为 MCP tools/call 的 error 文案。
#[derive(Debug, Clone)]
pub struct LspError(pub String);

impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LspError {}

/// 单次 LSP 请求往返超时（秒）。rust-analyzer 冷启动为秒级，30s 裕量足够。
pub(crate) const REQUEST_TIMEOUT_SECS: u64 = 30;

/// 单帧上限（64 MiB）：合法语言服务器响应远小于此。
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// 响应是否"有实质内容"（非 null 且非空数组）。
pub(crate) fn is_meaningful(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        _ => true,
    }
}

// ------------------------------------------------------------------ 分帧 --

/// 以 `Content-Length` 头写入一帧 JSON-RPC 消息。
pub(crate) async fn write_frame<W>(w: &mut W, msg: &Value) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(header.as_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// 读取一帧：逐行解析头直到空行，再精确读取 Content-Length 字节体。
///
/// `Ok(None)` 表示对端已关闭（EOF）。头名大小写不敏感；缺失
/// Content-Length 或体超限返回 `Err`。
pub(crate) async fn read_frame<R>(r: &mut R) -> io::Result<Option<Value>>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = r.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                if let Ok(len) = value.trim().parse::<usize>() {
                    content_length = Some(len);
                }
            }
        }
    }
    let len = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized LSP frame",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    let value = serde_json::from_slice(&body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(Some(value))
}

// ------------------------------------------------------------------- URI --

/// 路径 → `file://` URI（非保留字节百分号编码，保留 `/`）。
pub(crate) fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    #[cfg(windows)]
    {
        let path = without_verbatim_prefix(path);
        let text = path.to_string_lossy().replace('\\', "/");
        if let Some(unc) = text.strip_prefix("//") {
            push_uri_path(&mut out, unc);
        } else if matches!(text.as_bytes(), [b'A'..=b'Z' | b'a'..=b'z', b':', b'/', ..]) {
            out.push('/');
            out.push_str(&text[..2]); // 盘符的冒号是 URI 路径的一部分。
            push_uri_path(&mut out, &text[2..]);
        } else {
            push_uri_path(&mut out, &text);
        }
    }
    #[cfg(not(windows))]
    push_uri_path(&mut out, &path.to_string_lossy());
    out
}

fn push_uri_path(out: &mut String, path: &str) {
    for &byte in path.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
}

/// file URI → 路径（百分号解码）；非 file scheme 返回 None。
pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    #[cfg(not(windows))]
    {
        let decoded = percent_decode(rest);
        (!decoded.is_empty()).then(|| PathBuf::from(decoded))
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        if rest.contains(['?', '#', '\\']) {
            return None;
        }
        let (host, path) = rest.split_once('/')?;
        let path = percent_decode(path).replace('/', "\\");
        if path.contains('\0') {
            return None;
        }
        if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
            // 本机 URI 的首个 / 是 URI 分隔符，不是 Windows 路径的一部分。
            if matches!(
                path.as_bytes(),
                [b'A'..=b'Z' | b'a'..=b'z', b':', b'\\', ..]
            ) {
                return Some(PathBuf::from(path));
            }
            return None;
        }
        let host = percent_decode(host);
        if host.contains(['/', '\\', ':', '@', '\0']) || path.starts_with('\\') {
            return None;
        }
        let path = PathBuf::from(format!(r"\\{host}\{path}"));
        // UNC 必须有共享名，且不能被解释成设备或 verbatim 路径。
        match path.components().next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::UNC(_, share) if !share.is_empty()) => {
                Some(path)
            }
            _ => None,
        }
    }
}

#[cfg(windows)]
fn without_verbatim_prefix(path: &Path) -> std::borrow::Cow<'_, Path> {
    use std::path::{Component, Prefix};

    if let Some(Component::Prefix(prefix)) = path.components().next() {
        // canonicalize 返回的扩展长度前缀没有标准 file URI 表示。
        let text = path.to_string_lossy();
        match prefix.kind() {
            Prefix::VerbatimDisk(_) => return PathBuf::from(&text[4..]).into(),
            Prefix::VerbatimUNC(..) => return PathBuf::from(format!(r"\\{}", &text[8..])).into(),
            _ => {}
        }
    }
    path.into()
}

/// 响应 URI → 项目根相对路径；项目外目标（stdlib / 依赖源码等）返回
/// 解码后的绝对路径，避免静默丢弃跳转结果。
pub(crate) fn uri_to_display_path(uri: &str, root: &Path) -> Option<String> {
    let path = uri_to_path(uri)?;
    #[cfg(windows)]
    let root = without_verbatim_prefix(root);
    #[cfg(windows)]
    let root = root.as_ref();

    Some(match path.strip_prefix(root) {
        #[cfg(windows)]
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        #[cfg(not(windows))]
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ------------------------------------------------------------------ 解析 --

/// definition / references 响应（Location | Location[] | LocationLink |
/// LocationLink[] | null）→ 对外 Location 列表（wire 0-based line 转
/// 1-based），保持服务器顺序并去重。
pub(crate) fn parse_locations(result: &Value, root: &Path) -> Vec<Location> {
    let items: Vec<&Value> = match result {
        Value::Null => return Vec::new(),
        Value::Array(items) => items.iter().collect(),
        single => vec![single],
    };
    let mut out: Vec<Location> = Vec::new();
    for item in items {
        let Some((uri, pos)) = extract_link(item).or_else(|| extract_location(item)) else {
            continue;
        };
        let Some(file) = uri_to_display_path(&uri, root) else {
            continue;
        };
        let loc = Location {
            file,
            line: position_field(pos, "line").saturating_add(1),
            character: position_field(pos, "character"),
        };
        if !out.contains(&loc) {
            out.push(loc);
        }
    }
    out
}

/// LocationLink：优先 `targetSelectionRange`（符号名范围），退回 targetRange。
fn extract_link(item: &Value) -> Option<(String, &Value)> {
    let uri = item.get("targetUri")?.as_str()?.to_owned();
    let pos = item
        .get("targetSelectionRange")
        .and_then(|r| r.get("start"))
        .or_else(|| item.get("targetRange").and_then(|r| r.get("start")))?;
    Some((uri, pos))
}

/// Location：`uri` + `range.start`。
fn extract_location(item: &Value) -> Option<(String, &Value)> {
    let uri = item.get("uri")?.as_str()?.to_owned();
    let pos = item.get("range")?.get("start")?;
    Some((uri, pos))
}

fn position_field(pos: &Value, field: &str) -> u32 {
    pos.get(field).and_then(Value::as_u64).unwrap_or(0) as u32
}

/// hover 响应（null | {contents}）→ 纯文本。
///
/// contents 兼容 MarkupContent、MarkedString（string 或 {language, value}）
/// 与 MarkedString 数组三种形态；null 响应返回空串（无 hover 信息）。
pub(crate) fn parse_hover(result: &Value) -> String {
    let Some(contents) = result.get("contents") else {
        return String::new();
    };
    let mut out = String::new();
    append_marked(contents, &mut out);
    out.trim().to_owned()
}

fn append_marked(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(s);
        }
        Value::Array(items) => {
            for item in items {
                append_marked(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("value") {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(s);
            }
        }
        _ => {}
    }
}
