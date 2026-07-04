//! 内存 LRU 短期记忆 / in-memory LRU short-term memory.
//!
//! 用于缓存单会话内的近期上下文,容量满时淘汰最久未访问的记忆。
//! 不持久化,进程结束即消失。

use crate::{Memory, RecallQuery};

/// 内存 LRU 短期记忆 / in-memory LRU short-term memory.
#[derive(Debug, Clone)]
pub struct ShortTermMemory {
    capacity: usize,
    entries: Vec<Memory>,
}

impl ShortTermMemory {
    /// 创建指定容量的短期记忆 / create a short-term memory with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Vec::with_capacity(capacity.max(1)),
        }
    }

    /// 放入或刷新一条记忆 / put a memory, moving it to the front if it already exists.
    pub fn put(&mut self, mem: Memory) {
        if let Some(pos) = self.entries.iter().position(|m| m.id == mem.id) {
            self.entries.remove(pos);
        } else if self.entries.len() >= self.capacity {
            self.entries.pop();
        }
        self.entries.insert(0, mem);
    }

    /// 按文本子串与类别召回最近的记忆 / recall recent memories matching the query.
    pub fn recall(&self, query: &RecallQuery) -> Vec<Memory> {
        let q = query.query.to_lowercase();
        let mut hits: Vec<Memory> = self
            .entries
            .iter()
            .filter(|m| {
                let kind_ok = query.kind.map_or(true, |k| m.kind == k);
                kind_ok && m.content.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        hits.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_k = query.top_k.max(1);
        hits.into_iter().take(top_k).collect()
    }

    /// 返回当前条目数(用于测试与调试)/ return the current number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空 / is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ShortTermMemory {
    fn default() -> Self {
        Self::new(20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryKind;

    fn sample_memory(id: &str, content: &str, kind: MemoryKind) -> Memory {
        Memory {
            id: id.into(),
            kind,
            content: content.into(),
            embedding: None,
            weight: 1.0,
            created_at: 0,
            updated_at: 0,
            source: None,
        }
    }

    /// 验证 put 会按访问顺序维护 LRU。
    #[test]
    fn put_maintains_recency() {
        let mut stm = ShortTermMemory::new(2);
        stm.put(sample_memory("m1", "first", MemoryKind::ShortTerm));
        stm.put(sample_memory("m2", "second", MemoryKind::ShortTerm));
        stm.put(sample_memory("m1", "first updated", MemoryKind::ShortTerm));
        stm.put(sample_memory("m3", "third", MemoryKind::ShortTerm));

        assert_eq!(stm.len(), 2);
        // m1 被重新访问后回到队首,因此 m2 应被淘汰;最后放入的 m3 在最前。
        let recent: Vec<_> = stm.entries.iter().map(|m| m.id.clone()).collect();
        assert_eq!(recent, vec!["m3", "m1"]);
    }

    /// 验证 recall 按内容过滤并受 top_k 限制。
    #[test]
    fn recall_filters_and_respects_top_k() {
        let mut stm = ShortTermMemory::new(10);
        stm.put(sample_memory(
            "m1",
            "rust best practices",
            MemoryKind::Semantic,
        ));
        stm.put(sample_memory("m2", "python tips", MemoryKind::Semantic));
        stm.put(sample_memory("m3", "rust patterns", MemoryKind::Semantic));

        let hits = stm.recall(&RecallQuery {
            query: "rust".into(),
            kind: None,
            top_k: 2,
        });
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|m| m.content.contains("rust")));
    }

    /// 验证默认容量为 20。
    #[test]
    fn default_capacity_is_20() {
        let stm = ShortTermMemory::default();
        assert_eq!(stm.capacity, 20);
    }
}
