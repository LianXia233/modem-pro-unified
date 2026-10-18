//! 运行日志的增量推送器：只把**新增的完整行**推给在线页面。
//!
//! 日志是「一直在写」的数据，靠前端 5 秒轮询看就不像实时的（还白跑 HTTP），
//! 所以由后台循环每秒读一次增量广播；没有客户端时整轮跳过 —— 一个字节都不读
//! （与其它推送同一个原则：有人在看才干活）。
//!
//! 实现要点：
//!   · 首次读取**只建基线**（历史行由页面自己拉一次尾部），不补推；
//!   · 只推**完整行**（末尾没有换行的部分留到下一轮），否则页面上会出现半行；
//!   · 单次最多推 64KB，窗口从中间切进来时丢掉首行（大概率是半行）；
//!   · 识别日志轮转 / 截断：只看「文件变小」不够（截断后又写长可能反超旧 offset），
//!     所以记下 offset 之前一小段字节，下次校验它还在不在原处。

use std::fs;
use std::io::{Read, Seek, SeekFrom};

/// 单次最多推多少字节（长时间没人看之后别一次推巨量）。
pub const MAX_BYTES: usize = 64 * 1024;

/// 记住 offset 之前的一小段字节，用来识别文件被轮转 / 截断。
const SIG_BYTES: usize = 256;

/// 记住日志文件读到哪儿了，只把之后的新行推出去。
#[derive(Default)]
pub struct LogPusher {
    path: String,
    offset: u64,
    /// 是否已有基线（首次只记位置，历史行由页面自己拉）。
    ready: bool,
    /// offset 之前的 `SIG_BYTES` 字节（轮转检测用）。
    tail: Vec<u8>,
}

impl LogPusher {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            ..Self::default()
        }
    }

    /// 读一次新增的完整行；返回空表示「没有可推的」（首次建基线 / 无新增 / 只有半行）。
    pub fn read_new(&mut self) -> Vec<String> {
        let Ok(mut f) = fs::OpenOptions::new().read(true).open(&self.path) else {
            return Vec::new();
        };
        let Ok(size) = f.metadata().map(|m| m.len()) else {
            return Vec::new();
        };

        if !self.ready {
            // 首次只建基线：页面进来会自己拉一次尾部，不需要补推历史
            self.offset = size;
            self.ready = true;
            self.tail.clear();
            return Vec::new();
        }
        if size == self.offset {
            return Vec::new();
        }
        // 轮转 / 截断：整份重读
        if size < self.offset || !self.tail_ok(&mut f) {
            self.offset = 0;
            self.tail.clear();
        }

        let mut start = self.offset;
        let mut truncated = false;
        if size - start > MAX_BYTES as u64 {
            start = size - MAX_BYTES as u64;
            truncated = true;
        }
        if f.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut data = vec![0u8; (size - start) as usize];
        if f.read_exact(&mut data).is_err() {
            return Vec::new();
        }
        if truncated {
            // 窗口从中间切进来，首行大概率是半行：丢掉
            match data.iter().position(|b| *b == b'\n') {
                Some(i) => data.drain(..=i),
                None => return Vec::new(),
            };
        }
        // 只推完整行：末尾没换行的部分不推进 offset，留到下一轮
        let Some(last) = data.iter().rposition(|b| *b == b'\n') else {
            return Vec::new();
        };
        let consumed = &data[..=last];
        self.offset = start + consumed.len() as u64;
        self.remember_tail(consumed);

        let body = String::from_utf8_lossy(consumed);
        let body = body.strip_suffix('\n').unwrap_or(&body);
        body.split('\n')
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// 校验 offset 之前那段字节是否还在原处：不在就说明文件换了份。
    fn tail_ok(&self, f: &mut fs::File) -> bool {
        if self.offset == 0 || self.tail.is_empty() {
            return true;
        }
        let start = self.offset.saturating_sub(self.tail.len() as u64);
        let mut buf = vec![0u8; self.tail.len()];
        // 读不到就当「没变」处理，别乱重推
        let ok = f
            .seek(SeekFrom::Start(start))
            .and_then(|_| f.read_exact(&mut buf))
            .is_ok();
        !ok || buf == self.tail
    }

    fn remember_tail(&mut self, consumed: &[u8]) {
        let n = consumed.len().min(SIG_BYTES);
        self.tail.clear();
        self.tail.extend_from_slice(&consumed[consumed.len() - n..]);
    }
}

#[cfg(test)]
mod tests {
    use super::LogPusher;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 测试之间并行跑，临时目录必须各自独立（否则互相踩文件）。
    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn scratch(name: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "modemd-logtail-{}-{}-{}",
            std::process::id(),
            name,
            n
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("modemd.log")
    }

    fn write(path: &std::path::Path, text: &str) {
        std::fs::write(path, text.as_bytes()).unwrap();
    }

    /// 追加写（模拟日志一直在长）；`write` 是覆盖，会把前面的半行冲掉。
    fn append(path: &std::path::Path, text: &str) {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn first_read_only_builds_baseline() {
        let p = scratch("baseline");
        write(&p, "第一行\n第二行\n");

        let mut pusher = LogPusher::new(p.to_string_lossy().as_ref());
        // 首次只建基线，不补推历史
        assert!(pusher.read_new().is_empty());
        // 没有新增
        assert!(pusher.read_new().is_empty());

        // 追加两行：一次读出
        write(&p, "第一行\n第二行\n第三行\n第四行\n");
        assert_eq!(pusher.read_new(), vec!["第三行", "第四行"]);
        // 读完不重复推
        assert!(pusher.read_new().is_empty());

        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn partial_line_waits_for_next_round() {
        let p = scratch("partial");
        write(&p, "");

        let mut pusher = LogPusher::new(p.to_string_lossy().as_ref());
        assert!(pusher.read_new().is_empty());

        // 只写了半行：不推，也不推进 offset
        write(&p, "半");
        assert!(pusher.read_new().is_empty());
        // 补齐这一行（必须追加：write 会覆盖，前面的半行就没了）
        append(&p, "行\n");
        assert_eq!(pusher.read_new(), vec!["半行"]);

        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn rotation_restarts_from_beginning() {
        let p = scratch("rotation");
        write(&p, "旧文件第一行\n");
        let mut pusher = LogPusher::new(p.to_string_lossy().as_ref());
        assert!(pusher.read_new().is_empty());

        // 截断成更短的新内容（长度没超旧 offset，但签名对不上）
        write(&p, "新日志\n");
        assert_eq!(pusher.read_new(), vec!["新日志"]);

        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn crlf_lines_are_trimmed() {
        let p = scratch("crlf");
        write(&p, "");
        let mut pusher = LogPusher::new(p.to_string_lossy().as_ref());
        assert!(pusher.read_new().is_empty());
        write(&p, "windows 行\r\n");
        assert_eq!(pusher.read_new(), vec!["windows 行"]);

        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
