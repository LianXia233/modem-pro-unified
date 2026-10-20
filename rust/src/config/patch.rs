//! 原地改 `config.toml` 的单个键值（**保留注释与排版**）。
//!
//! 为什么不用序列化再写回：这个配置文件的注释比键还多（每一条都是真机踩坑的由来），
//! `toml::to_string` 一序列化就全没了。所以这里做**行级原地替换**：只动目标那一行，
//! 其它字节一个不动。
//!
//! 改前先备份 `config.toml.bak`（覆盖式，只留最近一份），改错还能捞回来。

use crate::error::{msg, Result};

/// 把 `[section]` 下的 `key` 设为**字符串**值（自动加引号并转义）。
pub fn set_string(path: &str, section: &str, key: &str, value: &str) -> Result<()> {
    write_value(path, section, key, &quote(value))
}

/// 把 `[section]` 下的 `key` 设为**原样**值（`true` / `false` / 数字这类，调用方自己负责写法）。
pub fn set_raw(path: &str, section: &str, key: &str, value: &str) -> Result<()> {
    write_value(path, section, key, value)
}

fn write_value(path: &str, section: &str, key: &str, literal: &str) -> Result<()> {
    let text =
        std::fs::read_to_string(path).map_err(|e| msg(format!("读取配置 {path} 失败: {e}")))?;
    let next = replace_value(&text, section, key, literal);
    if next == text {
        return Ok(());
    }
    backup(path);
    std::fs::write(path, next).map_err(|e| msg(format!("写回配置 {path} 失败: {e}")))?;
    Ok(())
}

/// 纯函数版替换（便于单测）：找不到就新增，段不存在就补一段。
///
/// `literal` 是**已经格式化好的值文本**（要不要引号由调用方决定）。
pub(crate) fn replace_value(text: &str, section: &str, key: &str, literal: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let value_line = format!("{key} = {literal}");

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    let mut in_section = false;
    let mut replaced = false;
    // 目标段里最后一个「非空、非注释」行的位置：新增的键插在它后面，
    // 免得掉进段尾那几行说明注释里（那样键虽然合法，读起来却像注释的一部分）
    let mut last_key_line: Option<usize> = None;

    for line in &lines {
        if let Some(name) = section_name(line) {
            if !replaced && in_section {
                // 段已结束仍未替换 ⇒ 插在这一段末尾
                let at = last_key_line.map(|i| i + 1).unwrap_or(out.len());
                out.insert(at, value_line.clone());
                replaced = true;
            }
            in_section = name == section;
            last_key_line = None;
            out.push((*line).to_string());
            continue;
        }
        if in_section && !replaced && is_key_line(line, key) {
            out.push(value_line.clone());
            replaced = true;
            continue;
        }
        if in_section && is_content_line(line) {
            last_key_line = Some(out.len());
        }
        out.push((*line).to_string());
    }
    if !replaced && in_section {
        let at = last_key_line.map(|i| i + 1).unwrap_or(out.len());
        out.insert(at, value_line.clone());
        replaced = true;
    }
    if !replaced {
        // 段都不存在：补在文件末尾
        if !out.is_empty() && !out.last().map(|s| s.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push(format!("[{section}]"));
        out.push(value_line);
    }

    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// 备份成 `<path>.bak`（失败不阻断：备份只是保险，写回才是正事）。
fn backup(path: &str) {
    let bak = format!("{path}.bak");
    if let Err(e) = std::fs::copy(path, &bak) {
        log::warn!("备份 {path} 到 {bak} 失败: {e}");
    }
}

/// 取 `[section]` 这样的段名（忽略注释行与行首空白）。
fn section_name(line: &str) -> Option<&str> {
    let t = line.trim();
    if !t.starts_with('[') || t.starts_with("#") {
        return None;
    }
    let end = t.find(']')?;
    Some(t[1..end].trim())
}

/// 是否是 `key = ...` 这一行（且没被注释掉）。
fn is_key_line(line: &str, key: &str) -> bool {
    let t = line.trim();
    if t.starts_with('#') {
        return false;
    }
    let Some(rest) = t.strip_prefix(key) else {
        return false;
    };
    rest.trim_start().starts_with('=')
}

/// 是否是「有内容」的行（非空、非纯注释）——用来找插入点。
fn is_content_line(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.starts_with('#') && !t.starts_with('[')
}

/// TOML 基本字符串的转义（引号 / 反斜杠 / 控制字符）。
fn quote(value: &str) -> String {
    let mut s = String::with_capacity(value.len() + 2);
    s.push('"');
    for ch in value.chars() {
        match ch {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04X}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

#[cfg(test)]
mod tests {
    use super::{quote, replace_value};

    const SAMPLE: &str = "\
# 顶部注释
[ui]
# 登录说明
require_luci_login = true
login_attempts = 5

[sms]
sms_memory = \"SM\"
";

    #[test]
    fn replaces_existing_key_and_keeps_comments() {
        let out = replace_value(SAMPLE, "ui", "require_luci_login", "false");
        assert!(out.contains("require_luci_login = false"));
        assert!(out.contains("# 登录说明"));
        assert!(out.contains("login_attempts = 5"));
        assert!(out.contains("sms_memory = \"SM\""));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn appends_key_when_missing_in_section() {
        let out = replace_value(SAMPLE, "sms", "send_queue_size", "8");
        assert!(out.contains("send_queue_size = 8"));
        // 插在段内最后一个键之后，不能掉到别的段里
        let ui_at = out.find("[ui]").unwrap();
        let sms_at = out.find("[sms]").unwrap();
        let key_at = out.find("send_queue_size").unwrap();
        assert!(ui_at < sms_at && sms_at < key_at);
    }

    #[test]
    fn appends_section_when_missing() {
        let out = replace_value(SAMPLE, "quota", "enabled", "true");
        assert!(out.contains("[quota]\nenabled = true"));
    }

    #[test]
    fn commented_out_key_is_not_touched() {
        let src = "[ui]\n# require_luci_login = false\n";
        let out = replace_value(src, "ui", "require_luci_login", "true");
        assert!(out.contains("# require_luci_login = false"));
        assert!(out.contains("\nrequire_luci_login = true"));
    }

    #[test]
    fn special_characters_are_escaped() {
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn no_trailing_newline_is_preserved() {
        let out = replace_value(
            "[ui]\nrequire_luci_login = true",
            "ui",
            "require_luci_login",
            "false",
        );
        assert!(out.ends_with("false"));
    }
}
