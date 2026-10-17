//! 文本子类型识别与富文本转纯文本（纯逻辑，便于单测）。
//!
//! 判定顺序：`url` > `email` > `color` > `path`。

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::db::models::ClipboardSubKind;

/// URL：要求带协议头（http/https/ftp/file）或 `www.` 开头的单行串。
/// 规则保持收紧，避免把任意带点的词误判为链接。
static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(https?|ftp|file)://[^\s]+$|^www\.[^\s]+\.[^\s]+$")
        .expect("invalid URL regex")
});

/// Email：本地部分允许字母、数字、中文及 `.` `+` `_` `-` `%`（RFC 5321 常见字符）。
static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9._%+\-\u{4e00}-\u{9fa5}]+@[a-zA-Z0-9_-]+(\.[a-zA-Z0-9_-]+)+$")
        .expect("invalid email regex")
});

/// Hex 颜色：#RGB / #RGBA / #RRGGBB / #RRGGBBAA。
static HEX_COLOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^#([0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$")
        .expect("invalid hex color regex")
});

/// 颜色函数：覆盖经典 `rgb()/rgba()/hsl()/hsla()`（含逗号 / 空格语法）
/// 以及 CSS Color 4/5：`hwb() / lab() / lch() / oklab() / oklch() / color() / color-mix()`。
/// 括号内不限定（含嵌套函数），由 [`is_safe_css_value`] 做安全 + 括号平衡校验。
static COLOR_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(rgba?|hsla?|hwb|lab|lch|oklab|oklch|color|color-mix)\(.+\)$")
        .expect("invalid color fn regex")
});

/// CSS 渐变：`linear-gradient(...)` / `radial-gradient(...)` / `conic-gradient(...)`
/// 及其 `repeating-*` 变体。括号内不限定内容，由 [`is_safe_css_value`] 做安全/平衡校验。
static GRADIENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(repeating-)?(linear|radial|conic)-gradient\(.+\)$")
        .expect("invalid gradient regex")
});

/// 识别纯文本的子类型。判定顺序：url > email > color > path。
/// 均不命中返回 `None`（普通文本）。
///
/// 注意：`path` 分支会触碰文件系统（`exists`），且仅认**绝对路径**——
/// 相对路径的存在性取决于进程 cwd（监听线程下不可控），收紧以避免误判。
pub fn detect_text_sub_kind(text: &str) -> Option<ClipboardSubKind> {
    let value = text.trim();
    if value.is_empty() {
        return None;
    }

    if URL_RE.is_match(value) {
        return Some(ClipboardSubKind::Url);
    }
    if EMAIL_RE.is_match(value) {
        return Some(ClipboardSubKind::Email);
    }
    if is_css_color_value(value) {
        return Some(ClipboardSubKind::Color);
    }
    if is_existing_absolute_path(value) {
        return Some(ClipboardSubKind::Path);
    }
    None
}

/// 把任意字符串规范化为可信的 CSS 颜色串：trim 后必须命中颜色 / 渐变规则
/// 且通过 [`is_safe_css_value`] 才返回 `Some`。命令层用它给前端 `colorPreview` 兜底，
/// 避免前端把任意文本塞进 CSS `background` 触发样式注入。
pub fn sanitize_css_color(text: &str) -> Option<String> {
    let value = text.trim();

    if value.is_empty() || !is_css_color_value(value) {
        return None;
    }

    Some(value.to_owned())
}

/// 统一判定：hex 直接命中即可（无括号无注入面）；
/// 函数式 / 渐变需要再过 [`is_safe_css_value`] 防注入与括号平衡。
fn is_css_color_value(value: &str) -> bool {
    if HEX_COLOR_RE.is_match(value) {
        return true;
    }

    if (COLOR_FN_RE.is_match(value) || GRADIENT_RE.is_match(value)) && is_safe_css_value(value) {
        return true;
    }

    false
}

/// CSS background 值安全校验：用于 gradient 这类括号内容自由的语法。
/// - 拒绝 `;` / `<` / `>` 等可能跳出声明 / 注入标签的字符；
/// - 拒绝 `url(` / `expression(` / `javascript:` / `@import` 等已知风险 token；
/// - 要求括号平衡，避免半截语法被浏览器宽松解析后吞掉后续 CSS。
fn is_safe_css_value(value: &str) -> bool {
    if value.contains(';') || value.contains('<') || value.contains('>') {
        return false;
    }

    let lower = value.to_ascii_lowercase();
    if lower.contains("url(")
        || lower.contains("expression(")
        || lower.contains("javascript:")
        || lower.contains("@import")
    {
        return false;
    }

    let mut depth: i32 = 0;
    for ch in value.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }

    depth == 0
}

fn is_existing_absolute_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute() && path.exists()
}

