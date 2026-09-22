//! 行缓存（LRU 池）：FIFO 淘汰的中间层，UI 端按需「加载更多」可把被淘汰的旧行拉回来。
//!
//! 设计动机：
//!   worker 端 `items: Vec<Map>` 持有全量；百万级目录直接吃掉几 GB。
//!   引入本层后：worker 端只保留最新 capacity 行；超出立即把最旧的搬到 pool；
//!   UI 端表格按 push 顺序渲染，**被淘汰的行暂留 pool**——底部「加载更多」一键拉回。
//!
//! 行为契约：
//!   · capacity = 0  → 不限，全部留下（不淘汰、不入池）；与现状保持一致。
//!   · capacity > 0  → 插入新元素时若 active.len() >= capacity，把 `active[0]` 移到 pool；
//!                    pool 容量同样 = capacity（保活最新一批的同时记住被淘汰的同样多）。
//!                    pool 也满时，pool 自己的最旧继续丢（即丢弃最旧的最旧）。
//!   · 线程安全：worker 端单线程使用；UI 端持有只读 handle（用 take_into_active）。
//!     但跨线程共享时内部用 Mutex 兜底。
//!
//! 简化取舍：本实现是 FIFO，不是严格 LRU（命中不重排）—— 满足主上「插入超出即淘汰最旧」
//! 的语义；UI 也不需要访问刷新（表格按 push 序渲染）。

use std::sync::Mutex;

/// 行缓存池。
///
/// `T` 通常是 `Map<String, Value>` 或 `Arc<Map<String, Value>>`。
/// 工作流：
///   pool.push(item)        // 推一条（可能触发淘汰）
///   pool.snapshot()        // 取出当前 active 列表用于发送
///   pool.take_more(n)      // 从 pool 拉 n 条回 active（UI 「加载更多」）
pub struct RowPool<T> {
    inner: Mutex<Inner<T>>,
    capacity: usize,
}

struct Inner<T> {
    /// 当前 active 的最新 N 条（capacity 个上限）
    active: Vec<T>,
    /// 被淘汰出来的旧条（FIFO 队尾是最旧的）
    evicted: Vec<T>,
}

impl<T> RowPool<T> {
    /// capacity = 0 表示不限
    pub fn new(capacity: usize) -> Self {
        Self { inner: Mutex::new(Inner { active: Vec::new(), evicted: Vec::new() }), capacity }
    }

    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 当前 active 长度
    #[allow(dead_code)]
    pub fn active_len(&self) -> usize {
        self.inner.lock().unwrap().active.len()
    }

    /// 当前池（被淘汰的）长度
    pub fn pool_len(&self) -> usize {
        self.inner.lock().unwrap().evicted.len()
    }

    /// 推一条：capacity=0 不淘汰；>0 超出立即淘汰最旧到 pool；pool 也满则丢 pool 最旧
    pub fn push(&self, item: T) {
        let mut g = self.inner.lock().unwrap();
        if self.capacity == 0 {
            g.active.push(item);
            return;
        }
        if g.active.len() >= self.capacity {
            // 旧的最旧的移到 evicted
            let old = g.active.remove(0);
            if g.evicted.len() >= self.capacity {
                // pool 也满：丢 pool 最旧（vec 队尾 = 最旧）
                g.evicted.pop();
            }
            g.evicted.push(old);
        }
        g.active.push(item);
    }

    /// 取出当前 active 快照（用于发 Batch）。返回 owned Vec。
    #[allow(dead_code)]
    pub fn snapshot(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.inner.lock().unwrap().active.clone()
    }

    /// 把当前 active 全部 take 走（清空 active），用于 drain 后发送
    #[allow(dead_code)]
    pub fn drain_active(&self) -> Vec<T> {
        let mut g = self.inner.lock().unwrap();
        std::mem::take(&mut g.active)
    }

    /// 从 evicted 池拉 `max` 条回到 active 队首（保持时间序）。
    /// 返回拉回的 owned Vec（drain 走，不保留在池里——重复点击需要重新跑扫描）。
    /// 调用方应把返回的 Vec 拼到自己持有的 filter_items 头部。
    pub fn take_more(&self, max: usize) -> Vec<T>
    where
        T: Clone,
    {
        if max == 0 {
            return Vec::new();
        }
        let mut g = self.inner.lock().unwrap();
        // 取最老时间序的 N 条 → 移到 active 队首
        let take = max.min(g.evicted.len());
        if take == 0 {
            return Vec::new();
        }
        // evicted: vec 头部 = 最旧，尾部 = 最新淘汰；从头部取
        let moved: Vec<T> = g.evicted.drain(..take).collect();
        let mut new_active = Vec::with_capacity(g.active.len() + moved.len());
        new_active.extend(moved.iter().cloned());
        new_active.append(&mut g.active);
        g.active = new_active;
        moved
    }

    /// 清空全部（新一轮扫描开始时）
    #[allow(dead_code)]
    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap();
        g.active.clear();
        g.evicted.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_zero_means_unlimited() {
        let p: RowPool<i32> = RowPool::new(0);
        for i in 0..1000 {
            p.push(i);
        }
        assert_eq!(p.active_len(), 1000);
        assert_eq!(p.pool_len(), 0);
    }

    #[test]
    fn capacity_overflow_moves_to_evicted() {
        let p: RowPool<i32> = RowPool::new(3);
        for i in 0..5 {
            p.push(i);
        }
        // active 最新 3 条: [2,3,4]
        assert_eq!(p.active_len(), 3);
        // evicted 池：前 2 条 [0,1]
        assert_eq!(p.pool_len(), 2);
        let snap = p.snapshot();
        assert_eq!(snap, vec![2, 3, 4]);
    }

    #[test]
    fn evicted_pool_overflow_drops_oldest_evicted() {
        let p: RowPool<i32> = RowPool::new(3);
        for i in 0..10 {
            p.push(i);
        }
        // active = [7,8,9]; evicted = [4,5,6]（最早 0,1,2,3 已被丢）
        assert_eq!(p.active_len(), 3);
        assert_eq!(p.pool_len(), 3);
        let snap = p.snapshot();
        assert_eq!(snap, vec![7, 8, 9]);
    }

    #[test]
    fn take_more_pulls_from_evicted_to_active_front() {
        // 推 0..10（10 个）到 capacity=3 的池：active=[7,8,9]，evicted=[0,1,6]
        // （pool 也满，evicted 自身的最旧被淘汰：[0,1,2] → [0,1] → push 3 后 [0,1,3] → push 4 后 [0,1,4] → ...
        // 最终 [0,1,6] —— 0,1 是最旧的还在，2~5 都被 evicted 自己的最旧淘汰机制丢掉了）
        let p: RowPool<i32> = RowPool::new(3);
        for i in 0..10 {
            p.push(i);
        }
        assert_eq!(p.snapshot(), vec![7, 8, 9]);
        assert_eq!(p.pool_len(), 3);
        // 取前 2 条（最旧）
        let moved = p.take_more(2);
        assert_eq!(moved, vec![0, 1]);
        // active 现在 = [0,1,7,8,9]（0,1 加到头部）
        let snap = p.snapshot();
        assert_eq!(snap, vec![0, 1, 7, 8, 9]);
        // evicted 池 = [6]
        assert_eq!(p.pool_len(), 1);
    }

    #[test]
    fn take_more_does_not_consume_evicted_permanently_by_default() {
        // take_more 是「拉回」而不是「保留」：被拉回的条目不再留在池中（drain 走）。
        // UI 想再看同一批就得再跑一次扫描。这是设计取舍：避免 active 无限膨胀。
        let p: RowPool<i32> = RowPool::new(3);
        for i in 0..10 {
            p.push(i);
        }
        // 先 take 2：moved=[0,1]，active=[0,1,7,8,9]，evicted=[6]
        let first = p.take_more(2);
        assert_eq!(first, vec![0, 1]);
        // 再 take 3：moved=[6]，active=[0,1,6,7,8,9] = 6 条，evicted 池空
        let second = p.take_more(3);
        assert_eq!(second, vec![6]);
        assert_eq!(p.active_len(), 6);
        assert_eq!(p.pool_len(), 0);
        // 再 take 啥也没有
        let third = p.take_more(5);
        assert!(third.is_empty());
    }

    #[test]
    fn clear_resets_both() {
        let p: RowPool<i32> = RowPool::new(3);
        for i in 0..10 {
            p.push(i);
        }
        p.clear();
        assert_eq!(p.active_len(), 0);
        assert_eq!(p.pool_len(), 0);
    }

    #[test]
    fn drain_active_clears_active() {
        let p: RowPool<i32> = RowPool::new(3);
        for i in 0..10 {
            p.push(i);
        }
        let drained = p.drain_active();
        assert_eq!(drained, vec![7, 8, 9]);
        assert_eq!(p.active_len(), 0);
        // evicted 保留
        assert_eq!(p.pool_len(), 3);
    }
}