//! 插件 console.* 的环形缓冲区。
//!
//! `console` 不再写 `plugins.log`（只有 `Coconut.log` 写文件），而是进这里；
//! 内置调试终端通过启动器本地 `/debug/console` 接口轮询读取。
//! 这是启动器内部通道，不是公开的 `Coconut` API。

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_ENTRIES: usize = 2000;

#[derive(Default)]
pub struct ConsoleBus {
    inner: Mutex<Inner>,
    seq: AtomicU64,
    cleared: AtomicU64,
}

#[derive(Default)]
struct Inner {
    entries: VecDeque<Value>,
}

impl ConsoleBus {
    pub fn push(&self, plugin: &str, level: &str, message: &str) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let entry = json!({
            "kind": "line",
            "seq": seq,
            "plugin": plugin,
            "level": level,
            "message": message,
        });
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.push_back(entry);
            while inner.entries.len() > MAX_ENTRIES {
                inner.entries.pop_front();
            }
        }
    }

    /// 清空终端。只清缓冲区并推进 cleared 序号；调用方自己决定是否重画。
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.clear();
        }
        self.cleared.fetch_add(1, Ordering::SeqCst);
    }

    pub fn snapshot(&self, since: u64) -> Value {
        let lines: Vec<Value> = self
            .inner
            .lock()
            .map(|inner| {
                inner
                    .entries
                    .iter()
                    .filter(|entry| entry["seq"].as_u64().unwrap_or(0) > since)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "seq": self.seq.load(Ordering::SeqCst),
            "cleared": self.cleared.load(Ordering::SeqCst),
            "lines": lines,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_lines_and_reports_clear() {
        let bus = ConsoleBus::default();
        bus.push("a", "info", "one");
        bus.push("b", "error", "two");
        let value = bus.snapshot(0);
        assert_eq!(value["lines"].as_array().unwrap().len(), 2);
        assert_eq!(value["cleared"], 0);
        bus.clear();
        let value = bus.snapshot(0);
        assert!(value["lines"].as_array().unwrap().is_empty());
        assert!(value["cleared"].as_u64().unwrap() > 0);
        bus.push("a", "info", "three");
        assert_eq!(bus.snapshot(0)["lines"].as_array().unwrap().len(), 1);
    }
}
