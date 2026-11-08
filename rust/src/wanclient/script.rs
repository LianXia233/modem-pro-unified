//! 上报脚本生成：把官方 `/lib/netifd/dhcp.script` / `dhcpv6.script` 复制一份，
//! 在每条 `proto_init_update "*" 0|1` 之后插入 `proto_set_keep 1`。
//!
//! 为什么不在仓库里放改好的脚本副本：OpenWrt 升级 netifd 后副本就过期了。
//! 旧 shim 用 `sed '/proto_init_update "\*" [01][[:space:]]*$/a proto_set_keep 1'`
//! 运行时生成；这里用等价的行级插入，逻辑收敛成一个纯函数以便单测。

use std::fs;
use std::io;
use std::path::Path;

/// 该行是否需要在其后插入 `proto_set_keep 1`。
///
/// 等价于 sed 的 `/proto_init_update "\*" [01][[:space:]]*$/`：
/// 行尾允许空白（含 `\r`，与 POSIX `[[:space:]]` 一致），但 `[01]` 之后必须直接到行尾。
pub fn needs_keep(line: &str) -> bool {
    let t = line.trim_end_matches([' ', '\t', '\r', '\x0b', '\x0c']);
    t.ends_with("proto_init_update \"*\" 0") || t.ends_with("proto_init_update \"*\" 1")
}

/// 对整份脚本文本插入 keep 标记，保留原有换行与行尾格式。
pub fn inject_keep(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    for line in src.split_inclusive('\n') {
        match line.strip_suffix('\n') {
            Some(body) => {
                out.push_str(body);
                out.push('\n');
                if needs_keep(body) {
                    out.push_str("proto_set_keep 1\n");
                }
            }
            None => {
                // 末尾没有换行的最后一行
                out.push_str(line);
                if needs_keep(line) {
                    out.push_str("\nproto_set_keep 1\n");
                }
            }
        }
    }
    out
}

/// 生成实例专属的上报脚本（权限 0755，与旧 shim 的 `chmod 0755` 一致）。
pub fn generate(src: &str, out: &Path) -> io::Result<()> {
    let text = fs::read_to_string(src)?;
    let patched = inject_keep(&text);
    if patched.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "生成的上报脚本为空",
        ));
    }
    fs::write(out, patched.as_bytes())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(out, fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
#!/bin/sh
[ -n \"$IFACE\" ] && proto_init_update \"*\" 1
proto_send_update \"$INTERFACE\"
[ -n \"$IFACE\" ] && proto_init_update \"*\" 0
proto_send_update \"$INTERFACE\"
";

    #[test]
    fn inserts_after_both_keep_states() {
        let out = inject_keep(SRC);
        assert_eq!(out.matches("proto_set_keep 1").count(), 2);
        assert_eq!(
            out,
            "\
#!/bin/sh
[ -n \"$IFACE\" ] && proto_init_update \"*\" 1
proto_set_keep 1
proto_send_update \"$INTERFACE\"
[ -n \"$IFACE\" ] && proto_init_update \"*\" 0
proto_set_keep 1
proto_send_update \"$INTERFACE\"
"
        );
    }

    #[test]
    fn allows_trailing_whitespace() {
        assert!(needs_keep("proto_init_update \"*\" 1  \t"));
        assert!(needs_keep("proto_init_update \"*\" 0\t"));
    }

    #[test]
    fn ignores_non_matching_lines() {
        assert!(!needs_keep("proto_init_update \"$iface\" 1"));
        assert!(!needs_keep("proto_init_update \"*\" 2"));
        assert!(!needs_keep("proto_init_update \"*\" 11"));
        assert!(!needs_keep("proto_init_update \"*\" 1 # keep"));
    }

    #[test]
    fn no_match_leaves_text_untouched() {
        let src = "#!/bin/sh\necho hi\n";
        assert_eq!(inject_keep(src), src);
    }

    #[test]
    fn handles_last_line_without_newline() {
        let out = inject_keep("proto_init_update \"*\" 1");
        assert_eq!(out, "proto_init_update \"*\" 1\nproto_set_keep 1\n");
    }

    #[test]
    fn keeps_indentation_of_following_lines() {
        let src = "  proto_init_update \"*\" 1\n\ttrue\n";
        let out = inject_keep(src);
        assert_eq!(
            out,
            "  proto_init_update \"*\" 1\nproto_set_keep 1\n\ttrue\n"
        );
    }

    #[test]
    fn generate_reports_missing_source() {
        let out = std::env::temp_dir().join("mp-dhcp-test-missing.script");
        let err = generate("/lib/netifd/dhcp.script.not-exist", &out).unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::InvalidData);
    }
}
