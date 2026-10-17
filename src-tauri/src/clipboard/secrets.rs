//! 高置信密钥 / token 检测。
//!
//! 规则刻意保守：只拦截带固定前缀、标准结构或明确私钥边界的内容，
//! 避免把普通长文本、颜色值、文件路径等误判为敏感凭据。

use regex::Regex;
use std::sync::OnceLock;

/// 判断文本是否包含高置信 secret/token，命中时整条剪贴板内容不入库。
pub fn contains_secret(text: &str) -> bool {
    let value = text.trim();
    if value.is_empty() {
        return false;
    }

    has_private_key_block(value)
        || has_known_prefixed_token(value)
        || has_aws_access_key(value)
        || has_jwt(value)
        || has_labeled_secret(value)
}

/// 识别 PEM/OpenSSH 私钥块，避免保存整段私钥。
fn has_private_key_block(text: &str) -> bool {
    static PRIVATE_KEY_RE: OnceLock<Regex> = OnceLock::new();
    PRIVATE_KEY_RE
        .get_or_init(|| {
            Regex::new(r"-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----").expect("valid regex")
        })
        .is_match(text)
}

/// 识别常见服务商自带前缀的 token。
fn has_known_prefixed_token(text: &str) -> bool {
    static PREFIXED_TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    PREFIXED_TOKEN_RE
        .get_or_init(|| {
            Regex::new(
                r"(?x)
                (?i:
                    \bgh[pousr]_[A-Za-z0-9_]{36,}\b
                  | \bgithub_pat_[A-Za-z0-9_]{40,}\b
                  | \bsk-[A-Za-z0-9_-]{32,}\b
                  | \bsk-proj-[A-Za-z0-9_-]{32,}\b
                  | \bxox[baprs]-[A-Za-z0-9-]{20,}\b
                  | \b(?:api|access|refresh|secret)[_-]?token_[A-Za-z0-9_-]{24,}\b
                )
                ",
            )
            .expect("valid regex")
        })
        .is_match(text)
}

/// 识别 AWS access key id。Secret access key 无固定前缀，单独识别会误伤，暂不拦截。
fn has_aws_access_key(text: &str) -> bool {
    static AWS_KEY_RE: OnceLock<Regex> = OnceLock::new();
    AWS_KEY_RE
        .get_or_init(|| Regex::new(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b").expect("valid regex"))
        .is_match(text)
}

/// 识别结构完整的 JWT：三段 base64url，header 解码后包含 typ/alg 常见字段。
fn has_jwt(text: &str) -> bool {
    static JWT_RE: OnceLock<Regex> = OnceLock::new();
    JWT_RE
        .get_or_init(|| {
            Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")
                .expect("valid regex")
        })
        .is_match(text)
}

/// 识别带明确字段名的 secret 赋值。字段名与分隔符之间允许一个可选的闭合引号，
/// 以覆盖 JSON / 引号包裹的配置（`"api_key": "..."`、`'secret_key': '...'`）。
fn has_labeled_secret(text: &str) -> bool {
    static LABELED_SECRET_RE: OnceLock<Regex> = OnceLock::new();
    LABELED_SECRET_RE
        .get_or_init(|| {
            Regex::new(
                r#"(?ix)
                \b
                (?:api[_-]?key|secret[_-]?key|client[_-]?secret|access[_-]?token|refresh[_-]?token|auth[_-]?token|bearer)
                \b
                ['"]?\s*[:=]\s*
                ['"]?
                [A-Za-z0-9][A-Za-z0-9._~+/=-]{23,}
                ['"]?
                "#,
            )
            .expect("valid regex")
        })
        .is_match(text)
}

